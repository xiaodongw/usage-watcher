//! OpenAI Codex.
//!
//! Usage comes from `GET /backend-api/wham/usage` ("wham" being Codex's
//! internal codename). This matters more than it looks: it is plain HTTP with a
//! bearer token, so it works anywhere — including Android, where neither the
//! `codex` binary nor its `app-server` exists. The app-server JSON-RPC route
//! returns the same data but needs the CLI installed and a supervised child
//! process, so it is only used in delegated mode.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::Adapter;
use crate::auth::{Credential, Flow, OAuthConfig, RedirectMode, TokenBody};
use crate::model::{AuthKind, Meter, Provider, Status};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

const CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const AUTHORIZE_URL: &str = "https://auth.openai.com/oauth/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/oauth/token";
/// Codex registers a fixed redirect, so this port is not negotiable.
const REDIRECT_PORT: u16 = 1455;

#[derive(Debug, Clone, Copy)]
pub struct Codex;

impl Adapter for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }

    fn label(&self) -> &'static str {
        "OpenAI Codex"
    }

    fn oauth_config(&self) -> Result<OAuthConfig> {
        Ok(OAuthConfig {
            flow: Flow::Oauth2,
            authorize_url: AUTHORIZE_URL.into(),
            token_url: TOKEN_URL.into(),
            client_id: CLIENT_ID.into(),
            scopes: vec![
                "openid".into(),
                "profile".into(),
                "email".into(),
                "offline_access".into(),
            ],
            redirect: RedirectMode::Loopback {
                port: REDIRECT_PORT,
                path: "/auth/callback".into(),
                // Codex shows no fallback code, so there is nothing to paste.
                allow_paste: false,
            },
            extra_authorize_params: vec![
                ("id_token_add_organizations".into(), "true".into()),
                ("codex_cli_simplified_flow".into(), "true".into()),
            ],
            // OpenAI's endpoint is ordinary RFC 6749.
            token_body: TokenBody::Form,
            exchange_echoes_state: false,
            refresh_scopes: Vec::new(),
        })
    }

    fn delegated_path(&self) -> Option<PathBuf> {
        Some(dirs::home_dir()?.join(".codex").join("auth.json"))
    }

    async fn fetch(
        &self,
        http: &reqwest::Client,
        cred: &Credential,
        kind: AuthKind,
    ) -> Result<Provider> {
        let account_id = cred.extra.get("account_id").context(
            "no ChatGPT account id on the Codex credential — the usage endpoint \
             requires it; sign in again to capture it from the id_token",
        )?;

        let resp = http
            .get(USAGE_URL)
            .bearer_auth(&cred.access_token)
            .header("chatgpt-account-id", account_id)
            .send()
            .await
            .context("could not reach the Codex usage endpoint")?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "usage endpoint returned {status}: {}",
                body.chars().take(300).collect::<String>()
            );
        }

        let usage: UsageResponse =
            serde_json::from_str(&body).context("unexpected shape from the usage endpoint")?;

        Ok(build(usage, kind))
    }
}

fn build(usage: UsageResponse, kind: AuthKind) -> Provider {
    let mut meters = Vec::new();

    if let Some(rl) = &usage.rate_limit {
        if let Some(w) = &rl.primary_window {
            meters.push(window_meter("primary", w));
        }
        if let Some(w) = &rl.secondary_window {
            meters.push(window_meter("secondary", w));
        }
    }

    if let Some(c) = &usage.credits {
        // `balance` arrives as a string, not a number.
        let amount = c.balance.as_deref().and_then(|b| b.parse::<f64>().ok());
        // Only show credits when they are actually part of this plan. A Plus
        // account reports `has_credits: false` with a "0" balance, and
        // rendering that as a critically-empty wallet is a false alarm.
        let in_use = c.has_credits || c.unlimited || amount.is_some_and(|a| a > 0.0);
        if let (Some(amount), true) = (amount, in_use) {
            meters.push(Meter::balance("credits", "Credits", amount, "USD", c.unlimited));
        }
    }

    Provider {
        id: "codex".into(),
        label: "OpenAI Codex".into(),
        plan: usage.plan_type,
        status: Status::Ok,
        auth: kind,
        updated_at: Utc::now(),
        meters,
    }
}

fn window_meter(id: &str, w: &Window) -> Meter {
    let mins = w.limit_window_seconds.map(|s| (s / 60) as u32);
    // Name the window by its real duration rather than assuming "5-hour" and
    // "weekly": this account currently reports only one 7-day bucket.
    let label = match mins {
        Some(m) if m % (60 * 24) == 0 => format!("{}-day", m / (60 * 24)),
        Some(m) if m % 60 == 0 => format!("{}-hour", m / 60),
        Some(m) => format!("{m}-min"),
        None => id.to_string(),
    };
    let resets_at = w.reset_at.and_then(|t| DateTime::from_timestamp(t, 0));
    Meter::window(id, &label, w.used_percent, resets_at, mins)
}

/// Read `~/.codex/auth.json`, read-only.
///
/// The file records `last_refresh` but no expiry, so the access token's own
/// `exp` claim is the only reliable source. As with Claude, the refresh token
/// is deliberately dropped so nothing downstream can rotate it.
pub fn read_delegated(path: &Path) -> Result<Credential> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("could not read {} — is Codex signed in?", path.display()))?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;

    let tokens = v.get("tokens").context("no `tokens` section in auth.json")?;

    let access_token = tokens
        .get("access_token")
        .and_then(|t| t.as_str())
        .context("auth.json has no access token")?
        .to_string();

    let mut extra = std::collections::HashMap::new();
    if let Some(acc) = tokens.get("account_id").and_then(|a| a.as_str()) {
        extra.insert("account_id".to_string(), acc.to_string());
    } else if let Some(acc) = tokens
        .get("id_token")
        .and_then(|t| t.as_str())
        .and_then(crate::auth::oauth::account_id_from_id_token)
    {
        extra.insert("account_id".to_string(), acc);
    }

    Ok(Credential {
        expires_at: super::jwt_expiry(&access_token),
        access_token,
        refresh_token: None,
        extra,
    })
}

/// Read `auth.json` *including* its refresh token, for `uw auth adopt`.
/// See the note on [`crate::providers::claude::read_full_credential`].
pub fn read_full_credential(path: &Path) -> Result<Credential> {
    let mut cred = read_delegated(path)?;

    let raw = std::fs::read_to_string(path)?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    let tokens = v.get("tokens").context("no `tokens` section in auth.json")?;

    cred.refresh_token = Some(
        tokens
            .get("refresh_token")
            .and_then(|t| t.as_str())
            .context("auth.json has no refresh token to adopt")?
            .to_string(),
    );
    Ok(cred)
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    plan_type: Option<String>,
    #[serde(default)]
    rate_limit: Option<RateLimit>,
    #[serde(default)]
    credits: Option<Credits>,
}

#[derive(Debug, Deserialize)]
struct RateLimit {
    #[serde(default)]
    primary_window: Option<Window>,
    #[serde(default)]
    secondary_window: Option<Window>,
}

#[derive(Debug, Deserialize)]
struct Window {
    used_percent: f32,
    #[serde(default)]
    limit_window_seconds: Option<i64>,
    #[serde(default)]
    reset_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct Credits {
    #[serde(default)]
    has_credits: bool,
    #[serde(default)]
    unlimited: bool,
    #[serde(default)]
    balance: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MeterKind, Severity};

    /// Captured verbatim from a real `/backend-api/wham/usage` response.
    const SAMPLE: &str = r#"{
      "plan_type": "plus",
      "rate_limit": {
        "allowed": true, "limit_reached": false,
        "primary_window": {"used_percent":19,"limit_window_seconds":604800,
                           "reset_after_seconds":333237,"reset_at":1787284072},
        "secondary_window": null
      },
      "credits": {"has_credits":false,"unlimited":false,"balance":"0"},
      "spend_control": {"reached": false},
      "rate_limit_reset_credits": {"available_count": 0}
    }"#;

    #[test]
    fn parses_real_payload() {
        let u: UsageResponse = serde_json::from_str(SAMPLE).unwrap();
        let p = build(u, AuthKind::OwnGrant);

        assert_eq!(p.plan.as_deref(), Some("plus"));
        // Just the one window: Plus reports no credits, and those are hidden.
        assert_eq!(p.meters.len(), 1);

        // 604800s must render as "7-day", not "10080-min".
        assert_eq!(p.meters[0].label, "7-day");
        match p.meters[0].kind {
            MeterKind::Window { used_pct, window_mins, resets_at } => {
                assert_eq!(used_pct, 19.0);
                assert_eq!(window_mins, Some(10080));
                assert_eq!(resets_at.unwrap().timestamp(), 1787284072);
            }
            _ => panic!("expected a window"),
        }
    }

    #[test]
    fn credits_not_part_of_the_plan_are_hidden() {
        // Plus reports has_credits:false with a "0" balance. Showing that as a
        // critically-empty wallet would be a permanent false alarm.
        let u: UsageResponse = serde_json::from_str(SAMPLE).unwrap();
        let p = build(u, AuthKind::OwnGrant);
        assert!(p.meters.iter().all(|m| m.id != "credits"));
    }

    #[test]
    fn real_credit_balance_is_shown_and_scored() {
        let json = r#"{"credits":{"has_credits":true,"unlimited":false,"balance":"2.50"}}"#;
        let u: UsageResponse = serde_json::from_str(json).unwrap();
        let p = build(u, AuthKind::OwnGrant);
        match &p.meters[0].kind {
            MeterKind::Balance { amount, .. } => assert_eq!(*amount, 2.50),
            _ => panic!("expected a balance"),
        }
        assert_eq!(p.meters[0].severity, Severity::Warning);
    }

    #[test]
    fn unlimited_credits_are_never_critical() {
        let json = r#"{"credits":{"unlimited":true,"balance":"0"}}"#;
        let u: UsageResponse = serde_json::from_str(json).unwrap();
        let p = build(u, AuthKind::OwnGrant);
        assert_eq!(p.meters[0].severity, Severity::Normal);
    }

    #[test]
    fn hour_windows_label_correctly() {
        let w = Window { used_percent: 50.0, limit_window_seconds: Some(18000), reset_at: None };
        assert_eq!(window_meter("primary", &w).label, "5-hour");
    }

    #[test]
    fn missing_rate_limit_yields_no_windows_not_an_error() {
        let u: UsageResponse = serde_json::from_str(r#"{"plan_type":"plus"}"#).unwrap();
        let p = build(u, AuthKind::Delegated);
        assert!(p.meters.is_empty());
    }
}
