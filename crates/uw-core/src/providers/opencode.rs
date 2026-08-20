//! opencode (Zen and Go).
//!
//! `GET https://opencode.ai/zen/go/v1/usage` is undocumented but real. It was
//! added by PR #16513 in the opencode repo, in answer to a request for exactly
//! what this crate does — a way to see Go plan headroom without opening the
//! dashboard — and it lives in the console app at
//! `packages/console/app/src/routes/zen/go/v1/usage.ts`. Probing bears that out:
//! any other path under `/zen/go/v1/` answers 404, while this one answers 401
//! for a missing key.
//!
//! Two things follow from it being a Go-plan route:
//!
//! - Zen proper (pay-as-you-go, `/zen/v1`) has no equivalent. `/zen/v1/usage` is
//!   a 404, and there is no balance endpoint either. A Zen-only key therefore
//!   gets an honest [`Status::Unavailable`] rather than an error.
//! - The route authenticates the API key directly against the console's key
//!   table, which means it works with the key the CLI already stored — no OAuth
//!   involved, and nothing to refresh.
//!
//! There is no own-grant flow: opencode issues keys from the web console only.
//! `uw auth token opencode` and `uw auth adopt opencode` both cover the case
//! where the CLI is not installed, which is what a phone needs.
//!
//! Being undocumented, the response shape moves: it has already gone from three
//! top-level `{status, resetInSec, usagePercent}` objects to a nested `usage`
//! map of `{status, percent, resetsAt}`. Everything here is therefore optional
//! and a 200 that yields no windows is treated as an error rather than as an
//! empty-but-fine reading — see [`build`]. Silently rendering "no meters" is how
//! the first version of this adapter hid exactly that change.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

use super::{Adapter, AuthPreference, Spec};
use crate::auth::{Credential, OAuthConfig};
use crate::model::{AuthKind, Meter, Provider, Severity, Status};

const USAGE_URL: &str = "https://opencode.ai/zen/go/v1/usage";

/// The provider ids opencode files its keys under, best first. `opencode-go` is
/// the subscription and the only one this endpoint can answer for; plain
/// `opencode` is Zen, kept as a fallback so a Zen-only user gets the accurate
/// "no usage API" tile rather than "no credential found".
const AUTH_KEYS: [&str; 2] = ["opencode-go", "opencode"];

#[derive(Debug, Clone, Copy)]
pub struct Opencode;

impl Adapter for Opencode {
    fn id(&self) -> &'static str {
        "opencode"
    }

    fn label(&self) -> &'static str {
        "opencode"
    }

    /// The message matters as much as the failure: it is what the config
    /// screen shows in place of a greyed-out "Sign in with your browser", so
    /// it has to name the alternative without assuming a terminal.
    fn oauth_config(&self) -> Result<OAuthConfig> {
        bail!(
            "opencode issues API keys from its web console rather than by OAuth, \
             so there is no browser sign-in. Borrow the opencode CLI's key, or \
             paste one from https://opencode.ai/zen"
        )
    }

    fn delegated_path(&self) -> Option<PathBuf> {
        // opencode follows the XDG data convention rather than a dotfile in
        // $HOME, so this is not `~/.opencode`.
        Some(
            dirs::data_dir()
                .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))?
                .join("opencode")
                .join("auth.json"),
        )
    }

    fn spec(&self) -> Spec {
        Spec::new(
            "opencode Zen Go — rolling, weekly and monthly usage windows.",
            "#6366f1",
        )
        .vendor_cli("opencode")
        .docs("https://opencode.ai/zen")
        .token(
            "Paste an API key",
            "API key",
            "sk-…",
            "Copy a key from the opencode Zen console.",
            Some("https://opencode.ai/zen"),
        )
    }

    fn read_delegated(&self, path: &Path) -> Result<Credential> {
        read_delegated(path)
    }

    fn read_full_credential(&self, path: &Path) -> Result<Credential> {
        read_full_credential(path)
    }

    /// A static key, so adopting it is a copy rather than a transfer — the
    /// opencode CLI carries on using its own and nothing rotates.
    fn adopt_as(&self) -> Option<AuthPreference> {
        Some(AuthPreference::Token)
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
            .send()
            .await
            .context("could not reach the opencode usage endpoint")?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        // 403 is the endpoint's way of saying "this key is not on Go". That is
        // a fact about the account, not a failure, and an error tile with a red
        // message would be wrong every time it appeared.
        if status == reqwest::StatusCode::FORBIDDEN {
            return Ok(unavailable(
                "This key is on opencode Zen, which is pay-as-you-go and publishes \
                 no usage or balance API. Only the Go subscription reports headroom.",
                kind,
            ));
        }

        if !status.is_success() {
            bail!(
                "usage endpoint returned {status}: {}",
                body.chars().take(300).collect::<String>()
            );
        }

        let usage: UsageResponse =
            serde_json::from_str(&body).context("unexpected shape from the usage endpoint")?;

        build(usage, kind)
    }
}

fn unavailable(reason: &str, kind: AuthKind) -> Provider {
    Provider {
        id: "opencode".into(),
        label: "opencode".into(),
        plan: None,
        status: Status::Unavailable {
            reason: reason.into(),
        },
        auth: kind,
        updated_at: Utc::now(),
        meters: Vec::new(),
    }
}

/// `Err` when the endpoint answered but said nothing we understand.
///
/// A 200 with no recognisable windows means the shape changed under us. Showing
/// that as a healthy provider with no rows is the worst of both worlds: it looks
/// deliberate, and it is indistinguishable from a plan that genuinely has no
/// limits. Fail, and the tile says what happened.
fn build(body: UsageResponse, kind: AuthKind) -> Result<Provider> {
    let usage = body.usage.unwrap_or_default();

    let meters: Vec<Meter> = [
        ("rolling", "Rolling", &usage.rolling),
        ("weekly", "Weekly", &usage.weekly),
        ("monthly", "Monthly", &usage.monthly),
    ]
    .iter()
    .filter_map(|(id, label, w)| w.as_ref().map(|w| window_meter(id, label, w)))
    .collect();

    if meters.is_empty() {
        bail!(
            "the opencode usage endpoint answered, but carried no rolling, weekly \
             or monthly window — its (undocumented) response shape has probably \
             changed again"
        );
    }

    Ok(Provider {
        id: "opencode".into(),
        label: "opencode".into(),
        plan: Some("go".into()),
        status: Status::Ok,
        auth: kind,
        updated_at: Utc::now(),
        meters,
    })
}

fn window_meter(id: &str, label: &str, w: &Window) -> Meter {
    // Already an instant, so the panel's countdown keeps ticking between polls
    // without the daemon having to re-poll to keep it honest. Anything already
    // in the past is noise rather than information.
    let resets_at = w.resets_at.filter(|t| *t > Utc::now());

    // The server's own verdict beats our percentage thresholds: `rate-limited`
    // means requests are being refused right now, whatever the number says.
    let mut m = Meter::window(id, label, w.percent, resets_at, None);
    if w.status == "rate-limited" {
        m.severity = Severity::Critical;
    }
    m
}

/// Read opencode's `auth.json`, read-only.
///
/// Entries are `{"<provider>": {"type": "api", "key": "..."}}`. These are static
/// API keys — no expiry, no refresh token — so unlike Claude and Codex there is
/// no rotation hazard in reading one, and nothing here can sign the CLI out.
pub fn read_delegated(path: &Path) -> Result<Credential> {
    let raw = std::fs::read_to_string(path).with_context(|| {
        format!(
            "could not read {} — is opencode signed in? (`opencode auth login`)",
            path.display()
        )
    })?;
    let v: serde_json::Value =
        serde_json::from_str(&raw).with_context(|| format!("{} is not valid JSON", path.display()))?;

    let key = AUTH_KEYS
        .iter()
        .find_map(|id| {
            let entry = v.get(id)?;
            // Only `type: "api"` carries a usable key; an oauth-shaped entry
            // would have a different set of fields entirely.
            match entry.get("type").and_then(|t| t.as_str()) {
                Some("api") => entry.get("key")?.as_str().map(str::to_string),
                _ => None,
            }
        })
        .with_context(|| {
            format!(
                "no opencode Zen or Go API key in {} — run `opencode auth login` \
                 and choose OpenCode Go, or `uw auth token opencode`",
                path.display()
            )
        })?;

    Ok(Credential {
        access_token: key,
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    })
}

/// The same read, for `uw auth adopt`.
///
/// Identical to [`read_delegated`] rather than a wider read, because there is
/// nothing wider to read: the key *is* the whole credential. Adopting it copies
/// a static key into our own store so the watcher keeps working where opencode
/// is not installed, and — unlike Claude and Codex — it does not require
/// re-running the vendor login, because nothing is being rotated.
pub fn read_full_credential(path: &Path) -> Result<Credential> {
    read_delegated(path)
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    #[serde(default)]
    usage: Option<Windows>,
}

#[derive(Debug, Default, Deserialize)]
struct Windows {
    #[serde(default)]
    rolling: Option<Window>,
    #[serde(default)]
    weekly: Option<Window>,
    #[serde(default)]
    monthly: Option<Window>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Window {
    /// `"ok"` or `"rate-limited"`.
    #[serde(default)]
    status: String,
    /// Was `usagePercent` before the route was reshaped; the alias costs
    /// nothing and means an older deployment still reads correctly.
    #[serde(alias = "usagePercent")]
    percent: f32,
    #[serde(default)]
    resets_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::MeterKind;

    /// The shape `packages/console/app/src/routes/zen/go/v1/usage.ts` returns
    /// today: a `usage` map of three windows, each `{status, percent, resetsAt}`
    /// as produced by its `formatUsage` helper.
    const SAMPLE: &str = r#"{"usage":{
      "rolling": {"status":"ok","percent":65,"resetsAt":"2030-01-01T02:30:00.000Z"},
      "weekly":  {"status":"ok","percent":30,"resetsAt":"2030-01-04T00:00:00.000Z"},
      "monthly": {"status":"ok","percent":12,"resetsAt":"2030-01-21T00:00:00.000Z"}
    }}"#;

    fn parse(json: &str) -> UsageResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn parses_all_three_windows() {
        let p = build(parse(SAMPLE), AuthKind::Delegated).unwrap();

        assert_eq!(p.plan.as_deref(), Some("go"));
        assert_eq!(
            p.meters.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(),
            ["Rolling", "Weekly", "Monthly"]
        );

        match p.meters[0].kind {
            MeterKind::Window {
                used_pct,
                window_mins,
                resets_at,
            } => {
                assert_eq!(used_pct, 65.0);
                // The endpoint gives a reset instant, never the window length,
                // and deriving one from a partly-elapsed window would be wrong.
                assert_eq!(window_mins, None);
                assert_eq!(
                    resets_at.unwrap().to_rfc3339(),
                    "2030-01-01T02:30:00+00:00"
                );
            }
            _ => panic!("expected a window"),
        }
    }

    /// The route was reshaped after it shipped — flat `usagePercent` fields
    /// became a nested `usage` map of `percent`. Reading the older deployment
    /// costs one serde alias.
    #[test]
    fn the_pre_reshape_percent_field_still_reads() {
        let p = build(
            parse(r#"{"usage":{"rolling":{"status":"ok","usagePercent":41}}}"#),
            AuthKind::Delegated,
        )
        .unwrap();
        match p.meters[0].kind {
            MeterKind::Window { used_pct, .. } => assert_eq!(used_pct, 41.0),
            _ => panic!("expected a window"),
        }
    }

    /// The regression that motivated `build` returning `Result`: the first
    /// version of this adapter parsed the pre-reshape shape, met the current
    /// one, found nothing, and rendered a healthy tile with no rows.
    #[test]
    fn a_200_we_cannot_read_is_an_error_not_an_empty_green_tile() {
        for body in [
            r#"{}"#,
            r#"{"usage":{}}"#,
            r#"{"rollingUsage":{"status":"ok","usagePercent":65}}"#,
        ] {
            let err = build(parse(body), AuthKind::Delegated)
                .unwrap_err()
                .to_string();
            assert!(err.contains("response shape"), "{body}: {err}");
        }
    }

    #[test]
    fn rate_limited_is_critical_whatever_the_percentage_says() {
        let p = build(
            parse(r#"{"usage":{"rolling":{"status":"rate-limited","percent":100}}}"#),
            AuthKind::Delegated,
        )
        .unwrap();
        assert_eq!(p.meters[0].severity, Severity::Critical);
    }

    #[test]
    fn a_window_the_endpoint_omits_gets_no_row() {
        let p = build(
            parse(r#"{"usage":{"rolling":{"status":"ok","percent":5}}}"#),
            AuthKind::Delegated,
        )
        .unwrap();
        assert_eq!(p.meters.len(), 1);
    }

    #[test]
    fn an_elapsed_reset_instant_is_dropped_rather_than_counted_down_from() {
        let p = build(
            parse(
                r#"{"usage":{"weekly":{"status":"ok","percent":40,
                    "resetsAt":"2020-01-01T00:00:00.000Z"}}}"#,
            ),
            AuthKind::Delegated,
        )
        .unwrap();
        match p.meters[0].kind {
            MeterKind::Window { resets_at, .. } => assert!(resets_at.is_none()),
            _ => panic!("expected a window"),
        }
    }

    #[test]
    fn prefers_the_go_key_over_the_zen_key() {
        let dir = std::env::temp_dir().join(format!("uw-opencode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        std::fs::write(
            &path,
            r#"{"lmstudio":{"type":"api","key":"local"},
                "opencode":{"type":"api","key":"zen-key"},
                "opencode-go":{"type":"api","key":"go-key"}}"#,
        )
        .unwrap();

        // Go is the only plan the usage endpoint can answer for, so it wins.
        let c = read_delegated(&path).unwrap();
        assert_eq!(c.access_token, "go-key");
        // A static key never expires and must never look refreshable.
        assert!(c.refresh_token.is_none());
        assert!(c.expires_at.is_none());
        assert!(!c.is_expired());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn falls_back_to_the_zen_key_and_ignores_unrelated_providers() {
        let dir = std::env::temp_dir().join(format!("uw-opencode-zen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        std::fs::write(
            &path,
            r#"{"anthropic":{"type":"oauth","refresh":"nope"},
                "opencode":{"type":"api","key":"zen-key"}}"#,
        )
        .unwrap();

        assert_eq!(read_delegated(&path).unwrap().access_token, "zen-key");

        // A file with no opencode key at all must say what to run, not panic.
        std::fs::write(&path, r#"{"lmstudio":{"type":"api","key":"local"}}"#).unwrap();
        let err = read_delegated(&path).unwrap_err().to_string();
        assert!(err.contains("opencode auth login"), "{err}");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn there_is_no_login_and_the_error_names_the_alternative() {
        // This string is not only a CLI error: it is what the config screen
        // shows in place of a greyed-out "Sign in with your browser", so it
        // has to point somewhere without assuming a terminal.
        let err = Opencode.oauth_config().unwrap_err().to_string();
        assert!(err.contains("https://opencode.ai/zen"), "{err}");
        assert!(!err.contains("uw auth"), "assumes a terminal: {err}");
    }

    #[test]
    fn the_manifest_offers_a_paste_but_never_a_browser_login() {
        use crate::providers::LoginKind;
        let info = crate::providers::Any::Opencode(Opencode).info();
        let browser = info.methods.iter().find(|m| m.kind == LoginKind::Browser).unwrap();
        assert!(!browser.available);
        assert!(browser.unavailable_reason.is_some());

        let paste = info.methods.iter().find(|m| m.kind == LoginKind::Paste).unwrap();
        assert!(paste.available);
        assert!(paste.token.is_some(), "a paste method with no field to paste into");
    }
}
