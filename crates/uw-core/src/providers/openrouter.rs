//! OpenRouter.
//!
//! Unlike Claude and Codex, there is no vendor CLI here to borrow a credential
//! from, so own-grant is the only sensible default. It is also the easiest of
//! the four: OpenRouter documents a PKCE flow whose whole purpose is letting a
//! third-party app mint a key on the user's behalf, and the key it returns is
//! durable — no refresh, no expiry, nothing to rotate. That makes it the one
//! provider whose credential is trivially portable to a phone.

use anyhow::{bail, Context, Result};
use chrono::Utc;
use serde::Deserialize;
use std::path::PathBuf;

use super::{Adapter, AuthPreference, Spec};
use crate::auth::{Credential, Flow, OAuthConfig, RedirectMode, TokenBody};
use crate::limits::rate_limited;
use crate::model::{AuthKind, Meter, MeterKind, Period, Provider, Severity, Status};

/// Works with any inference key, and is the only endpoint that always answers.
const KEY_URL: &str = "https://openrouter.ai/api/v1/key";
/// The account-wide wallet. Newer OpenRouter accounts restrict this to
/// provisioning keys, so a failure here is not a failure of the poll.
const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";

const AUTHORIZE_URL: &str = "https://openrouter.ai/auth";
const EXCHANGE_URL: &str = "https://openrouter.ai/api/v1/auth/keys";

#[derive(Debug, Clone, Copy)]
pub struct OpenRouter;

impl Adapter for OpenRouter {
    fn id(&self) -> &'static str {
        "openrouter"
    }

    fn label(&self) -> &'static str {
        "OpenRouter"
    }

    /// No vendor CLI exists, so borrowing a credential is not an option and
    /// the global `delegated` default would only ever produce an error tile.
    fn default_auth(&self) -> AuthPreference {
        AuthPreference::Own
    }

    fn oauth_config(&self) -> Result<OAuthConfig> {
        Ok(OAuthConfig {
            flow: Flow::OpenRouterKey,
            authorize_url: AUTHORIZE_URL.into(),
            token_url: EXCHANGE_URL.into(),
            // Unused by this flow — OpenRouter has no client registration, and
            // the PKCE verifier is the only thing binding the code to us.
            client_id: String::new(),
            scopes: Vec::new(),
            redirect: RedirectMode::Loopback {
                port: 0,
                path: "/callback".into(),
                // The page redirects unconditionally; there is no code to paste.
                allow_paste: false,
            },
            extra_authorize_params: Vec::new(),
            // Inert under `Flow::OpenRouterKey`, which sends a fixed JSON body
            // and never refreshes. Listed for the compiler, not for the wire.
            token_body: TokenBody::Json,
            exchange_echoes_state: false,
            refresh_scopes: Vec::new(),
        })
    }

    fn delegated_path(&self) -> Option<PathBuf> {
        None
    }

    fn spec(&self) -> Spec {
        Spec::new(
            "OpenRouter — credits left on the account, and any cap on the key \
             itself.",
            include_bytes!("icons/openrouter.png"),
        )
        .docs("https://openrouter.ai/docs")
        .token(
            "Paste an API key",
            "API key",
            "sk-or-v1-…",
            "Create a key in your OpenRouter dashboard.",
            Some("https://openrouter.ai/settings/keys"),
        )
    }

    /// The slowest of the four. A prepaid balance only moves when you spend,
    /// and the account wallet is not a per-request counter.
    fn poll_intervals(&self) -> (u64, u64) {
        (300, 900)
    }

    async fn fetch(
        &self,
        http: &reqwest::Client,
        cred: &Credential,
        kind: AuthKind,
    ) -> Result<Provider> {
        let key = get_json::<KeyEnvelope>(http, KEY_URL, &cred.access_token)
            .await
            .context("could not read the OpenRouter key's usage")?
            .data;

        // Best effort. `/credits` is the account wallet and an inference key is
        // increasingly not allowed to see it; that must degrade one meter, not
        // the whole tile, since `/key` already carries the numbers that matter
        // for a key with a spend cap.
        let credits = get_json::<CreditsEnvelope>(http, CREDITS_URL, &cred.access_token)
            .await
            .ok()
            .map(|c| c.data);

        Ok(build(key, credits, kind))
    }
}

async fn get_json<T: serde::de::DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<T> {
    let resp = http
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .with_context(|| format!("could not reach {url}"))?;

    let status = resp.status();
    let limited = rate_limited(status, resp.headers(), "OpenRouter");
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        if let Some(limited) = limited {
            return Err(limited.into());
        }
        bail!(
            "{url} returned {status}: {}",
            body.chars().take(300).collect::<String>()
        );
    }
    serde_json::from_str(&body).with_context(|| format!("unexpected shape from {url}"))
}

fn build(key: KeyData, credits: Option<CreditsData>, kind: AuthKind) -> Provider {
    let mut meters = Vec::new();

    // The wallet, when we are allowed to see it. This is the number that
    // actually answers "how much have I got left".
    if let Some(c) = &credits {
        let remaining = c.total_credits - c.total_usage;
        meters.push(Meter {
            id: "credits".into(),
            label: "Credits".into(),
            kind: MeterKind::Balance {
                amount: remaining,
                currency: "USD".into(),
                // Bounded by what was purchased, so the bar has a real scale.
                of_total: (c.total_credits > 0.0).then_some(c.total_credits),
                unlimited: false,
            },
            severity: Severity::from_balance(remaining, false),
        });
    }

    // A per-key spend cap, if this key has one. Distinct from the wallet: the
    // key can be capped at $5 while the account holds $200, and it is the cap
    // that stops your requests.
    if let (Some(limit), Some(remaining)) = (key.limit, key.limit_remaining) {
        meters.push(Meter {
            id: "key_limit".into(),
            label: "Key limit".into(),
            kind: MeterKind::Balance {
                amount: remaining,
                currency: "USD".into(),
                of_total: (limit > 0.0).then_some(limit),
                unlimited: false,
            },
            severity: Severity::from_balance(remaining, false),
        });
    }

    // Context rather than a limit, so it is only worth a row once there is
    // something to show. Never scored — spending money is not a fault
    // condition, and a permanently-warning row teaches you to ignore the tile.
    if key.usage_monthly > 0.0 {
        meters.push(Meter {
            id: "spend_month".into(),
            label: "This month".into(),
            kind: MeterKind::Spend {
                amount: key.usage_monthly,
                currency: "USD".into(),
                period: Period::Monthly,
            },
            severity: Severity::Normal,
        });
    }

    // Free-tier keys have no wallet and no cap, so the tile would otherwise be
    // empty and look broken. Say so instead.
    let status = if meters.is_empty() {
        Status::Unavailable {
            reason: "This key has no credit balance and no spend limit, so there \
                     is no headroom to report."
                .into(),
        }
    } else {
        Status::Ok
    };

    Provider {
        id: "openrouter".into(),
        label: "OpenRouter".into(),
        plan: Some(if key.is_free_tier { "free" } else { "paid" }.into()),
        status,
        auth: kind,
        updated_at: Utc::now(),
        meters,
    }
}

#[derive(Debug, Deserialize)]
struct KeyEnvelope {
    data: KeyData,
}

#[derive(Debug, Deserialize)]
struct KeyData {
    /// Null when the key is uncapped, which is the common case.
    #[serde(default)]
    limit: Option<f64>,
    #[serde(default)]
    limit_remaining: Option<f64>,
    #[serde(default)]
    usage_monthly: f64,
    #[serde(default)]
    is_free_tier: bool,
}

#[derive(Debug, Deserialize)]
struct CreditsEnvelope {
    data: CreditsData,
}

#[derive(Debug, Deserialize)]
struct CreditsData {
    #[serde(default)]
    total_credits: f64,
    #[serde(default)]
    total_usage: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(json: &str) -> KeyData {
        serde_json::from_str::<KeyEnvelope>(json).unwrap().data
    }

    /// The documented shape of `GET /api/v1/key`.
    const KEY_SAMPLE: &str = r#"{"data":{
        "label":"usage-watcher","limit":null,"limit_reset":null,"limit_remaining":null,
        "include_byok_in_limit":false,"usage":41.5,"usage_daily":1.25,
        "usage_weekly":6.5,"usage_monthly":22.75,"byok_usage":0,"byok_usage_daily":0,
        "byok_usage_weekly":0,"byok_usage_monthly":0,"is_free_tier":false}}"#;

    #[test]
    fn credits_become_a_bounded_balance() {
        let c = CreditsData {
            total_credits: 100.5,
            total_usage: 25.75,
        };
        let p = build(key(KEY_SAMPLE), Some(c), AuthKind::OwnGrant);

        let m = p.meters.iter().find(|m| m.id == "credits").unwrap();
        match &m.kind {
            MeterKind::Balance {
                amount, of_total, ..
            } => {
                assert!((amount - 74.75).abs() < 1e-9);
                // Bounded, so the bar has something to fill against.
                assert_eq!(*of_total, Some(100.5));
            }
            _ => panic!("expected a balance"),
        }
        assert_eq!(m.severity, Severity::Normal);
    }

    #[test]
    fn a_nearly_empty_wallet_is_critical() {
        let c = CreditsData {
            total_credits: 20.0,
            total_usage: 19.5,
        };
        let p = build(key(KEY_SAMPLE), Some(c), AuthKind::OwnGrant);
        assert_eq!(p.meters[0].severity, Severity::Critical);
    }

    #[test]
    fn an_uncapped_key_reports_no_key_limit_row() {
        // `limit: null` is the normal case, and rendering it as a full or empty
        // bar would both be lies.
        let p = build(key(KEY_SAMPLE), None, AuthKind::OwnGrant);
        assert!(p.meters.iter().all(|m| m.id != "key_limit"));
    }

    #[test]
    fn a_capped_key_reports_its_own_remaining() {
        let k = key(
            r#"{"data":{"limit":10.0,"limit_remaining":2.5,"usage_monthly":7.5,
                        "is_free_tier":false}}"#,
        );
        let p = build(k, None, AuthKind::OwnGrant);
        let m = p.meters.iter().find(|m| m.id == "key_limit").unwrap();
        match &m.kind {
            MeterKind::Balance {
                amount, of_total, ..
            } => {
                assert_eq!(*amount, 2.5);
                assert_eq!(*of_total, Some(10.0));
            }
            _ => panic!("expected a balance"),
        }
        // $2.50 left on the cap is the warning band, same scale as everywhere.
        assert_eq!(m.severity, Severity::Warning);
    }

    #[test]
    fn losing_the_credits_endpoint_still_produces_a_usable_tile() {
        // `/credits` now wants a provisioning key on many accounts. That must
        // cost one row, not the whole provider.
        let k = key(r#"{"data":{"limit":10.0,"limit_remaining":9.0,"usage_monthly":1.0}}"#);
        let p = build(k, None, AuthKind::OwnGrant);
        assert_eq!(p.status, Status::Ok);
        assert!(p.meters.iter().any(|m| m.id == "key_limit"));
    }

    #[test]
    fn monthly_spend_is_context_and_never_raises_an_alarm() {
        let p = build(key(KEY_SAMPLE), None, AuthKind::OwnGrant);
        let m = p.meters.iter().find(|m| m.id == "spend_month").unwrap();
        match &m.kind {
            MeterKind::Spend { amount, period, .. } => {
                assert_eq!(*amount, 22.75);
                assert_eq!(*period, Period::Monthly);
            }
            _ => panic!("expected a spend"),
        }
        // Spending money is not a fault; a row that is always warning is a row
        // you stop reading.
        assert_eq!(m.severity, Severity::Normal);
    }

    #[test]
    fn zero_spend_gets_no_row() {
        let k = key(r#"{"data":{"usage_monthly":0,"is_free_tier":true}}"#);
        let p = build(k, None, AuthKind::OwnGrant);
        assert!(p.meters.iter().all(|m| m.id != "spend_month"));
    }

    #[test]
    fn a_free_key_with_nothing_to_measure_says_so_rather_than_looking_broken() {
        let k = key(r#"{"data":{"usage_monthly":0,"is_free_tier":true}}"#);
        let p = build(k, None, AuthKind::OwnGrant);
        assert!(p.meters.is_empty());
        assert!(matches!(p.status, Status::Unavailable { .. }));
        assert_eq!(p.plan.as_deref(), Some("free"));
    }

    #[test]
    fn own_grant_is_the_default_because_there_is_no_cli_to_borrow_from() {
        assert_eq!(OpenRouter.default_auth(), AuthPreference::Own);
        assert!(OpenRouter.delegated_path().is_none());
    }
}
