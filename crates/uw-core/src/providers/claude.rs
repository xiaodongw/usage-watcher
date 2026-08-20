//! Claude Code.
//!
//! Usage comes from `GET /api/oauth/usage`, the same endpoint Claude Code's own
//! `/usage` command calls. It is not a documented public API, so the adapter
//! drives off the generic `limits[]` array and tolerates unknown members rather
//! than hard-failing when Anthropic adds a bucket.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::{Adapter, AuthPreference, Spec};
use crate::auth::{Credential, Flow, OAuthConfig, RedirectMode, TokenBody};
use crate::limits::rate_limited;
use crate::model::{AuthKind, Meter, MeterKind, Provider, Severity, Status};

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const OAUTH_BETA: &str = "oauth-2025-04-20";

/// The Claude Code OAuth client.
///
/// Note this is *not* the Client ID Metadata Document URL published at
/// `claude.ai/oauth/claude-code-client-metadata` — that document belongs to the
/// MCP-connector client. The authorize endpoint rejects a URL outright
/// ("client_id: Input should be a valid UUID"), and this is the UUID the CLI
/// itself sends. There is no third-party registration for consumer-subscription
/// data, so own-grant mode necessarily presents as Claude Code.
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
/// The subscription ("claude.ai") authorize endpoint. Note this is **not**
/// `claude.ai/oauth/authorize` — that host renders a consent screen but then
/// fails the exchange with "Invalid request format". Claude Code's own config
/// names two distinct endpoints, and this is the one for Pro/Max logins:
/// `CONSOLE_AUTHORIZE_URL` (platform.claude.com) is for Console/API accounts.
const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
/// The CLI binds an ephemeral loopback port and redirects to
/// `http://localhost:<port>/callback`. Captured from the real
/// `claude auth login` flow — the hosted `platform.claude.com/oauth/code/callback`
/// page is *not* the registered redirect, and sending it is what produced
/// "Invalid request format".
const REDIRECT_PATH: &str = "/callback";

/// The scopes sent with a refresh — `$9e` in the CLI's own source.
const REFRESH_SCOPES: [&str; 5] = [
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

/// Authorize asks for those plus the Console pair `[org:create_api_key,
/// user:profile]`, deduped and in that order — `gRs = eo([...wRy, ...$9e])`
/// in the CLI. Both lists are transcribed from the binary rather than guessed;
/// the authorize endpoint reports any disagreement only as "Invalid request
/// format".
const AUTHORIZE_SCOPES: [&str; 6] = [
    "org:create_api_key",
    "user:profile",
    "user:inference",
    "user:sessions:claude_code",
    "user:mcp_servers",
    "user:file_upload",
];

#[derive(Debug, Clone, Copy)]
pub struct Claude;

impl Adapter for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn oauth_config(&self) -> Result<OAuthConfig> {
        Ok(OAuthConfig {
            flow: Flow::Oauth2,
            authorize_url: AUTHORIZE_URL.into(),
            token_url: TOKEN_URL.into(),
            client_id: CLIENT_ID.into(),
            scopes: AUTHORIZE_SCOPES.iter().map(|s| s.to_string()).collect(),
            redirect: RedirectMode::Loopback {
                port: 0,
                path: REDIRECT_PATH.into(),
                // Under WSL the Windows browser may not reach our listener, in
                // which case the page shows a code instead. Accept both.
                allow_paste: true,
            },
            // `code=true` is what makes that fallback code appear.
            extra_authorize_params: vec![("code".into(), "true".into())],
            // Anthropic's token endpoint is part of its JSON API, not a
            // form-encoded OAuth endpoint. Posting a form gets the generic
            // "Invalid request format".
            token_body: TokenBody::Json,
            exchange_echoes_state: true,
            // The CLI narrows the scope set on refresh: `org:create_api_key`
            // is an authorize-time concern only.
            refresh_scopes: REFRESH_SCOPES.iter().map(|s| s.to_string()).collect(),
        })
    }

    fn delegated_path(&self) -> Option<PathBuf> {
        Some(dirs::home_dir()?.join(".claude").join(".credentials.json"))
    }

    fn spec(&self) -> Spec {
        Spec::new(
            "Claude Code — the 5-hour session window, the weekly limits, and \
             the per-model weekly caps.",
            include_bytes!("icons/claude.png"),
        )
        .vendor_cli("Claude Code")
        .docs("https://claude.com/product/claude-code")
        .token(
            "Paste a long-lived token",
            "Token",
            "sk-ant-oat01-…",
            "Run `claude setup-token` and paste what it prints — a one-year \
             token, and the path Anthropic documents for machines with no browser.",
            None,
        )
    }

    fn read_delegated(&self, path: &Path) -> Result<Credential> {
        read_delegated(path)
    }

    fn read_full_credential(&self, path: &Path) -> Result<Credential> {
        read_full_credential(path)
    }

    fn adopt_as(&self) -> Option<AuthPreference> {
        Some(AuthPreference::Own)
    }

    fn relogin_hint(&self) -> Option<&'static str> {
        Some("claude auth login")
    }

    /// The plan name is not on the usage endpoint and not in the token
    /// response; Claude Code gets it from a separate profile call, which is
    /// what `user:profile` is for. Delegated mode reads the equivalent
    /// `subscriptionType` out of the CLI's credential file instead.
    async fn enrich(&self, http: &reqwest::Client, cred: &mut Credential) -> Result<()> {
        let profile: Profile = http
            .get(PROFILE_URL)
            .bearer_auth(&cred.access_token)
            .send()
            .await
            .context("could not reach the profile endpoint")?
            .error_for_status()?
            .json()
            .await
            .context("unexpected shape from the profile endpoint")?;

        if let Some(plan) = profile
            .organization
            .and_then(|o| o.organization_type)
            .and_then(|t| plan_name(&t))
        {
            cred.extra.insert("subscription_type".into(), plan.into());
        }
        Ok(())
    }

    async fn fetch(
        &self,
        http: &reqwest::Client,
        cred: &Credential,
        kind: AuthKind,
    ) -> Result<Provider> {
        let resp = http
            .get(USAGE_URL)
            .bearer_auth(&cred.access_token)
            .header("anthropic-beta", OAUTH_BETA)
            .send()
            .await
            .context("could not reach the Anthropic usage endpoint")?;

        let status = resp.status();
        // Read before `text()` consumes the response. This endpoint is limited
        // per IP rather than per account — an unauthenticated request draws the
        // same 429 — and it answers with an hour, which is four times our own
        // longest backoff. Ignoring it means never waiting long enough to be
        // let back in.
        let limited = rate_limited(status, resp.headers(), "The Anthropic usage endpoint");
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            if let Some(limited) = limited {
                return Err(limited.into());
            }
            // A token can be perfectly valid and still be refused here. The
            // one-year token from `claude setup-token` is the common case: it
            // carries `user:inference` only, so it can make model requests but
            // cannot read usage. Say that plainly instead of dumping JSON.
            if status == reqwest::StatusCode::FORBIDDEN && body.contains("scope requirement") {
                bail!(
                    "this token is valid but lacks the `user:profile` scope that the usage \
                     endpoint requires. A `claude setup-token` token only grants \
                     `user:inference`. Run `uw auth login claude` for a full OAuth grant instead."
                );
            }
            bail!(
                "usage endpoint returned {status}: {}",
                body.chars().take(300).collect::<String>()
            );
        }

        let usage: UsageResponse =
            serde_json::from_str(&body).context("unexpected shape from the usage endpoint")?;

        // The usage endpoint does not report the plan; it is only known from
        // whichever credential we hold.
        Ok(build(usage, kind, cred.extra.get("subscription_type").cloned()))
    }
}

fn build(usage: UsageResponse, kind: AuthKind, plan: Option<String>) -> Provider {
    let mut meters: Vec<Meter> = usage.limits.iter().map(limit_to_meter).collect();

    // Only surface paid overage when it is actually switched on; otherwise it
    // is noise on every tile.
    if let Some(extra) = &usage.extra_usage {
        if extra.is_enabled {
            if let Some(used) = extra.used_credits {
                let limit = extra.monthly_limit.unwrap_or(0.0);
                meters.push(Meter {
                    id: "extra_usage".into(),
                    label: "Extra usage".into(),
                    kind: MeterKind::Balance {
                        amount: (limit - used).max(0.0),
                        currency: extra.currency.clone().unwrap_or_else(|| "USD".into()),
                        of_total: extra.monthly_limit,
                        unlimited: false,
                    },
                    severity: Severity::from_balance((limit - used).max(0.0), false),
                });
            }
        }
    }

    Provider {
        id: "claude".into(),
        label: "Claude Code".into(),
        plan,
        status: Status::Ok,
        auth: kind,
        updated_at: Utc::now(),
        meters,
    }
}

fn limit_to_meter(l: &Limit) -> Meter {
    // `weekly_scoped` entries carry the model they apply to; without the label
    // three identical "weekly" rows would be indistinguishable.
    let label = match (l.kind.as_str(), l.scope.as_ref().and_then(|s| s.model.as_ref())) {
        ("session", _) => "5-hour".to_string(),
        ("weekly_all", _) => "7-day".to_string(),
        (_, Some(m)) => format!("weekly · {}", m.display_name.as_deref().unwrap_or("scoped")),
        (k, None) => k.replace('_', " "),
    };

    Meter::window(&l.kind, &label, l.percent, l.resets_at, None)
}

/// Read `~/.claude/.credentials.json`.
///
/// Read-only by construction: we take a copy of the token and never write back.
/// The refresh token is deliberately *not* carried into the [`Credential`], so
/// nothing downstream can attempt a refresh and invalidate Claude Code's own
/// session.
pub fn read_delegated(path: &Path) -> Result<Credential> {
    let raw = std::fs::read_to_string(path).with_context(|| {
        format!(
            "could not read {} — is Claude Code signed in?",
            path.display()
        )
    })?;
    let v: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("{} is not valid JSON", path.display()))?;

    let oauth = v
        .get("claudeAiOauth")
        .context("no `claudeAiOauth` section in the credentials file")?;

    let access_token = oauth
        .get("accessToken")
        .and_then(|t| t.as_str())
        .context("credentials file has no access token")?
        .to_string();

    let expires_at = oauth
        .get("expiresAt")
        .and_then(|e| e.as_i64())
        .and_then(DateTime::from_timestamp_millis);

    // The plan is recorded here, not on the usage endpoint, so carry it across.
    let mut extra = std::collections::HashMap::new();
    if let Some(t) = oauth.get("subscriptionType").and_then(|s| s.as_str()) {
        extra.insert("subscription_type".to_string(), t.to_string());
    }

    Ok(Credential {
        access_token,
        refresh_token: None,
        expires_at,
        extra,
    })
}

/// Read the credential file *including* its refresh token, for `uw auth adopt`.
///
/// Separate from [`read_delegated`] on purpose. Delegated mode must never see a
/// refresh token — dropping it there is what guarantees no code path can
/// rotate Claude Code's session by accident. Adoption is the one deliberate
/// exception, and the caller is expected to re-run `claude auth login`
/// afterwards so the CLI holds a grant of its own.
pub fn read_full_credential(path: &Path) -> Result<Credential> {
    let mut cred = read_delegated(path)?;

    let raw = std::fs::read_to_string(path)?;
    let v: serde_json::Value = serde_json::from_str(&raw)?;
    let oauth = v
        .get("claudeAiOauth")
        .context("no `claudeAiOauth` section in the credentials file")?;

    cred.refresh_token = Some(
        oauth
            .get("refreshToken")
            .and_then(|t| t.as_str())
            .context("credentials file has no refresh token to adopt")?
            .to_string(),
    );
    Ok(cred)
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    limits: Vec<Limit>,
    #[serde(default)]
    extra_usage: Option<ExtraUsage>,
}

#[derive(Debug, Deserialize)]
struct Limit {
    kind: String,
    percent: f32,
    #[serde(default)]
    resets_at: Option<DateTime<Utc>>,
    #[serde(default)]
    scope: Option<Scope>,
}

#[derive(Debug, Deserialize)]
struct Scope {
    #[serde(default)]
    model: Option<Model>,
}

#[derive(Debug, Deserialize)]
struct Model {
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExtraUsage {
    #[serde(default)]
    is_enabled: bool,
    #[serde(default)]
    monthly_limit: Option<f64>,
    #[serde(default)]
    used_credits: Option<f64>,
    #[serde(default)]
    currency: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from a real response.
    const SAMPLE: &str = r#"{
      "subscription_type": "pro",
      "limits": [
        {"kind":"session","group":"session","percent":28,"severity":"normal",
         "resets_at":"2026-08-17T10:00:00.807179+00:00","scope":null,"is_active":true},
        {"kind":"weekly_all","group":"weekly","percent":13,"severity":"normal",
         "resets_at":"2026-08-24T02:00:00.807204+00:00","scope":null,"is_active":false},
        {"kind":"weekly_scoped","group":"weekly","percent":22,"severity":"normal",
         "resets_at":"2026-08-24T01:59:59.807399+00:00",
         "scope":{"model":{"id":null,"display_name":"Fable"},"surface":null},"is_active":false}
      ],
      "extra_usage": {"is_enabled": false, "monthly_limit": null, "used_credits": null},
      "spend": {"percent": 0}
    }"#;

    #[test]
    fn parses_real_payload() {
        let u: UsageResponse = serde_json::from_str(SAMPLE).unwrap();
        let p = build(u, AuthKind::OwnGrant, Some("pro".into()));

        assert_eq!(p.plan.as_deref(), Some("pro"));
        assert_eq!(p.meters.len(), 3, "disabled extra_usage must not add a meter");

        let labels: Vec<_> = p.meters.iter().map(|m| m.label.as_str()).collect();
        assert_eq!(labels, ["5-hour", "7-day", "weekly · Fable"]);

        match p.meters[0].kind {
            MeterKind::Window { used_pct, resets_at, .. } => {
                assert_eq!(used_pct, 28.0);
                assert!(resets_at.is_some());
            }
            _ => panic!("session limit should be a window"),
        }
    }

    #[test]
    fn unknown_limit_kinds_still_render() {
        let json = r#"{"limits":[{"kind":"monthly_experimental","percent":7}]}"#;
        let u: UsageResponse = serde_json::from_str(json).unwrap();
        let p = build(u, AuthKind::Delegated, None);
        // A bucket we've never seen must degrade to a readable label, not panic.
        assert_eq!(p.meters[0].label, "monthly experimental");
    }

    #[test]
    fn severity_tracks_percentage() {
        let json = r#"{"limits":[
            {"kind":"session","percent":28},
            {"kind":"weekly_all","percent":85},
            {"kind":"weekly_scoped","percent":97}]}"#;
        let u: UsageResponse = serde_json::from_str(json).unwrap();
        let p = build(u, AuthKind::OwnGrant, Some("pro".into()));
        assert_eq!(p.meters[0].severity, Severity::Normal);
        assert_eq!(p.meters[1].severity, Severity::Warning);
        assert_eq!(p.meters[2].severity, Severity::Critical);
    }

    #[test]
    fn delegated_read_never_exposes_a_refresh_token() {
        let dir = std::env::temp_dir().join("uw-claude-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("creds.json");
        std::fs::write(
            &path,
            r#"{"claudeAiOauth":{"accessToken":"at","refreshToken":"MUST_NOT_LEAK",
                "expiresAt":4102444800000,"subscriptionType":"pro"}}"#,
        )
        .unwrap();

        let c = read_delegated(&path).unwrap();
        assert_eq!(c.access_token, "at");
        // This is the guard against signing the user out of Claude Code.
        assert!(c.refresh_token.is_none());
        assert!(!c.is_expired());
        // The plan is only knowable from the credential, so it must survive.
        assert_eq!(c.extra.get("subscription_type").map(String::as_str), Some("pro"));

        std::fs::remove_file(&path).ok();
    }
}

#[cfg(test)]
mod cli_parity {
    use super::*;
    use crate::auth::OAuthClient;

    /// Pins our authorize URL to `buildAuthUrl` as it appears in the Claude
    /// Code binary (2.1.235). Transcribed from the source, which appends, in
    /// order: `code`, `client_id`, `response_type`, `redirect_uri`, `scope`,
    /// `code_challenge`, `code_challenge_method`, `state`.
    ///
    /// The endpoint's only complaint about any deviation is "Invalid request
    /// format", so a mismatch here is very expensive to diagnose in the wild.
    /// `orgUUID`, `login_hint` and `login_method` are the CLI's remaining
    /// optional appends; it omits all three on a plain login, so we do too.
    #[test]
    fn authorize_url_matches_claude_codes_own_builder() {
        let url = OAuthClient::new(Claude.oauth_config().unwrap())
            .authorize_url("http://localhost:38569/callback", "CHALLENGE", "STATE")
            .unwrap();

        assert_eq!(
            url,
            "https://claude.com/cai/oauth/authorize\
             ?code=true\
             &client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e\
             &response_type=code\
             &redirect_uri=http%3A%2F%2Flocalhost%3A38569%2Fcallback\
             &scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference\
             +user%3Asessions%3Aclaude_code+user%3Amcp_servers+user%3Afile_upload\
             &code_challenge=CHALLENGE\
             &code_challenge_method=S256\
             &state=STATE"
        );
    }

    /// The refresh grant deliberately asks for less than the authorize grant.
    #[test]
    fn refresh_drops_the_api_key_scope() {
        let cfg = Claude.oauth_config().unwrap();
        assert!(cfg.scopes.contains(&"org:create_api_key".to_string()));
        assert!(!cfg.refresh_scopes.contains(&"org:create_api_key".to_string()));
        assert_eq!(cfg.refresh_scopes.len(), 5);
    }
}

#[derive(Debug, Deserialize)]
struct Profile {
    #[serde(default)]
    organization: Option<ProfileOrg>,
}

#[derive(Debug, Deserialize)]
struct ProfileOrg {
    #[serde(default)]
    organization_type: Option<String>,
}

/// `claude_max` → `max`, and so on. Transcribed from the CLI's own map; an
/// unrecognised type yields no plan rather than a raw internal identifier on
/// the tile.
fn plan_name(organization_type: &str) -> Option<&'static str> {
    match organization_type {
        "claude_max" => Some("max"),
        "claude_pro" => Some("pro"),
        "claude_enterprise" => Some("enterprise"),
        "claude_team" => Some("team"),
        _ => None,
    }
}

#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn maps_the_organization_types_the_cli_knows() {
        assert_eq!(plan_name("claude_max"), Some("max"));
        assert_eq!(plan_name("claude_pro"), Some("pro"));
        assert_eq!(plan_name("claude_team"), Some("team"));
        assert_eq!(plan_name("claude_enterprise"), Some("enterprise"));
    }

    /// Better a blank plan than `claude_something_new` rendered as a plan name.
    #[test]
    fn unknown_organization_type_yields_no_plan() {
        assert_eq!(plan_name("claude_something_new"), None);
    }

    #[test]
    fn profile_without_an_organization_parses() {
        let p: Profile = serde_json::from_str(r#"{"account":{"uuid":"x"}}"#).unwrap();
        assert!(p.organization.is_none());
    }
}
