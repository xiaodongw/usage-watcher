//! Generic OAuth 2.0 authorization-code + PKCE client.
//!
//! Shared by every `OwnGrant` provider. Provider-specific knowledge lives in
//! [`OAuthConfig`] only, so adding a provider never means touching this file.

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::loopback::Loopback;
use super::pkce::{random_state, Pkce};

/// How the authorization code gets back to us.
///
/// The two providers differ, and neither choice is ours to make — each is
/// whatever the provider registered for its client id.
#[derive(Debug, Clone)]
pub enum RedirectMode {
    /// We listen on loopback and the browser redirects straight to us.
    /// Codex registers a fixed port; `0` asks the OS for an ephemeral one.
    ///
    /// `allow_paste` additionally races a pasted code against the listener.
    /// Claude's `code=true` parameter makes its page display a code when the
    /// browser cannot reach our listener — common under WSL, SSH and
    /// containers — so both returns must be accepted for the same flow.
    Loopback {
        port: u16,
        path: String,
        allow_paste: bool,
    },
    /// The provider redirects to its own hosted page, which displays the code
    /// for the user to paste back. Claude Code works this way — it registers
    /// no loopback URI at all, so there is nothing for us to listen on.
    HostedPaste { redirect_uri: String },
}

/// How the token endpoint wants its request body.
///
/// RFC 6749 says form-encoded, and Codex follows it. Anthropic's endpoint is
/// part of its JSON API and answers a form body with the generic
/// `{"type":"error","error":{"type":"invalid_request_error",
/// "message":"Invalid request format"}}` — the same unhelpful message the
/// authorize endpoint gives, which is why this took so long to isolate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenBody {
    Form,
    Json,
}

/// Which shape of PKCE flow the provider implements.
///
/// Both shapes are "browser, PKCE, code back to a loopback port". They diverge
/// in what the authorize page expects and in what the exchange hands back, and
/// the divergence is wide enough that pretending otherwise would mean four more
/// booleans on [`OAuthConfig`] and a reader who cannot tell which combinations
/// are real.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// RFC 6749 authorization-code + PKCE. Claude and Codex.
    Oauth2,
    /// OpenRouter's key exchange. The authorize page takes `callback_url` plus
    /// the PKCE challenge and nothing else — no client registration, no scopes,
    /// no `state` — and the exchange mints a durable API key rather than a
    /// token pair, so there is never anything to refresh.
    OpenRouterKey,
}

/// Everything that differs between providers.
#[derive(Debug, Clone)]
pub struct OAuthConfig {
    pub flow: Flow,
    pub authorize_url: String,
    pub token_url: String,
    pub client_id: String,
    pub scopes: Vec<String>,
    pub redirect: RedirectMode,
    /// Extra query parameters the provider's authorize endpoint expects.
    pub extra_authorize_params: Vec<(String, String)>,
    pub token_body: TokenBody,
    /// Echo `state` back on the code exchange. Not in RFC 6749 — Anthropic's
    /// client sends it, so we match rather than guess whether it is checked.
    pub exchange_echoes_state: bool,
    /// Scopes sent with a refresh. Anthropic's client narrows the set here
    /// (it drops `org:create_api_key`); most providers send none at all.
    pub refresh_scopes: Vec<String>,
}

/// How a login surfaces to the user. The CLI prints and reads stdin; the
/// daemon hands the URL to whichever viewer asked and waits.
///
/// `Send + Sync` because the daemon runs a login on a spawned task, and a
/// `&dyn LoginUi` held across an await would otherwise make that future
/// non-`Send`. Costs the terminal implementation nothing.
pub trait LoginUi: Send + Sync {
    /// Present the authorize URL (open a browser, print it, or both).
    fn open(&self, url: &str) -> Result<()>;
    /// Ask the user to paste the code, blocking. Used by `HostedPaste`.
    fn read_code(&self) -> Result<String>;

    /// A non-blocking source of a pasted code, raced against the loopback
    /// listener. Returning `None` disables pasting for that login.
    fn paste_channel(&self) -> Option<tokio::sync::oneshot::Receiver<String>> {
        None
    }

    /// Async replacement for [`Self::read_code`], for UIs that must not block.
    ///
    /// Distinct from [`Self::paste_channel`], which is *optional* — an extra
    /// way to finish a loopback login when the browser cannot reach us. This
    /// one is the only way a [`RedirectMode::HostedPaste`] flow ever gets its
    /// code, so a UI that implements it must eventually deliver or drop the
    /// sender.
    ///
    /// The default keeps the blocking path, which is right for a terminal:
    /// `uw` has nothing else to do while it waits on stdin. A daemon serving
    /// other viewers does, and cannot park a runtime thread on a human.
    fn code_channel(&self) -> Option<tokio::sync::oneshot::Receiver<String>> {
        None
    }
}

/// A live credential. `refresh_token` is absent for providers that hand back a
/// non-expiring key (OpenRouter) rather than an OAuth token pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Provider-specific extras, e.g. Codex's `chatgpt-account-id`.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub extra: std::collections::HashMap<String, String>,
}

impl Credential {
    /// We refresh 60s early so a poll never races an expiry mid-flight.
    const SKEW: i64 = 60;

    pub fn is_expired(&self) -> bool {
        match self.expires_at {
            Some(exp) => Utc::now() + ChronoDuration::seconds(Self::SKEW) >= exp,
            None => false,
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    id_token: Option<String>,
}

pub struct OAuthClient {
    cfg: OAuthConfig,
    http: reqwest::Client,
}

impl OAuthClient {
    pub fn new(cfg: OAuthConfig) -> Self {
        OAuthClient {
            cfg,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Run the full interactive login, ending with a stored-ready credential.
    pub async fn login(&self, ui: &dyn LoginUi) -> Result<Credential> {
        let pkce = Pkce::generate();
        let state = random_state();

        match &self.cfg.redirect {
            RedirectMode::Loopback {
                port,
                path,
                allow_paste,
            } => {
                let loopback = Loopback::bind(*port, path).await?;
                let redirect_uri = loopback.redirect_uri();
                ui.open(&self.authorize_url(&redirect_uri, &pkce.challenge, &state)?)?;

                let wait = Duration::from_secs(300);
                let paste = if *allow_paste { ui.paste_channel() } else { None };
                // `None` for flows that define no `state`; see `Loopback::wait`.
                let expect = match self.cfg.flow {
                    Flow::Oauth2 => Some(state.as_str()),
                    Flow::OpenRouterKey => None,
                };

                let code = match paste {
                    None => {
                        loopback
                            .wait(expect, wait)
                            .await
                            .context("the browser never completed the redirect")?
                            .code
                    }
                    // Whichever return path the browser took, take it.
                    Some(rx) => tokio::select! {
                        r = loopback.wait(expect, wait) => {
                            r.context("the browser never completed the redirect")?.code
                        }
                        p = rx => {
                            let (code, returned_state) = split_pasted_code(&p?);
                            if let Some(rs) = returned_state {
                                if rs != state {
                                    bail!(
                                        "state mismatch: the pasted code did not come from \
                                         the login we started. Aborting."
                                    );
                                }
                            }
                            code
                        }
                    },
                };

                self.exchange(&code, &redirect_uri, &pkce.verifier, &state).await
            }
            RedirectMode::HostedPaste { redirect_uri } => {
                ui.open(&self.authorize_url(redirect_uri, &pkce.challenge, &state)?)?;

                let pasted = match ui.code_channel() {
                    Some(rx) => rx.await.context("the login was abandoned")?,
                    None => ui.read_code()?,
                };
                let (code, returned_state) = split_pasted_code(&pasted);

                // The hosted page appends the state as a fragment. When it is
                // present we must still check it; when the user pasted only the
                // code, there is nothing to check against.
                if let Some(rs) = returned_state {
                    if rs != state {
                        bail!(
                            "state mismatch: the pasted code did not come from the \
                             login we started. Aborting."
                        );
                    }
                }

                self.exchange(&code, redirect_uri, &pkce.verifier, &state).await
            }
        }
    }

    pub(crate) fn authorize_url(
        &self,
        redirect_uri: &str,
        challenge: &str,
        state: &str,
    ) -> Result<String> {
        let mut url = url::Url::parse(&self.cfg.authorize_url)?;
        {
            let mut q = url.query_pairs_mut();
            for (k, v) in &self.cfg.extra_authorize_params {
                q.append_pair(k, v);
            }
            match self.cfg.flow {
                // Order mirrors what the Claude Code CLI emits byte-for-byte.
                // It should not matter to a conforming server, but this
                // endpoint answers "Invalid request format" to anything it
                // dislikes without saying what, so matching exactly removes
                // one variable.
                Flow::Oauth2 => {
                    q.append_pair("client_id", &self.cfg.client_id);
                    q.append_pair("response_type", "code");
                    q.append_pair("redirect_uri", redirect_uri);
                    q.append_pair("scope", &self.cfg.scopes.join(" "));
                    q.append_pair("code_challenge", challenge);
                    q.append_pair("code_challenge_method", Pkce::METHOD);
                    q.append_pair("state", state);
                }
                // Three parameters, and `callback_url` rather than
                // `redirect_uri`. Sending the RFC 6749 set as well would not
                // help: there is no client to identify and no scope to ask for.
                Flow::OpenRouterKey => {
                    q.append_pair("callback_url", redirect_uri);
                    q.append_pair("code_challenge", challenge);
                    q.append_pair("code_challenge_method", Pkce::METHOD);
                }
            }
        }
        Ok(url.to_string())
    }

    async fn exchange(
        &self,
        code: &str,
        redirect_uri: &str,
        verifier: &str,
        state: &str,
    ) -> Result<Credential> {
        if self.cfg.flow == Flow::OpenRouterKey {
            return self.post_key_exchange(code, verifier).await;
        }

        let mut body = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", self.cfg.client_id.as_str()),
            ("code_verifier", verifier),
        ];
        if self.cfg.exchange_echoes_state {
            body.push(("state", state));
        }
        self.post_token(&body).await
    }

    /// Exchange a refresh token for a fresh access token.
    ///
    /// Callers must persist the returned credential *before* using its access
    /// token: if the provider rotated the refresh token and we crash in
    /// between, the old one is already dead and the account is locked out.
    pub async fn refresh(&self, refresh_token: &str) -> Result<Credential> {
        if self.cfg.flow == Flow::OpenRouterKey {
            // Unreachable in practice — a key with no expiry is never seen as
            // stale — but a silent RFC 6749 refresh against a key-exchange
            // endpoint would fail in a far more confusing way than this.
            bail!("this provider issues durable API keys; there is nothing to refresh");
        }
        let scope = self.cfg.refresh_scopes.join(" ");
        let mut body = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", self.cfg.client_id.as_str()),
        ];
        if !scope.is_empty() {
            body.push(("scope", scope.as_str()));
        }
        let mut cred = self.post_token(&body).await?;
        // Providers that don't rotate omit the field; carry the old one forward
        // rather than losing the ability to refresh again.
        if cred.refresh_token.is_none() {
            cred.refresh_token = Some(refresh_token.to_string());
        }
        Ok(cred)
    }

    /// OpenRouter's exchange: PKCE proof in, a durable API key out.
    ///
    /// The key has no expiry and no refresh token, so the credential we build
    /// is deliberately bare — `is_expired()` is then permanently false and
    /// nothing ever tries to rotate it.
    async fn post_key_exchange(&self, code: &str, verifier: &str) -> Result<Credential> {
        let resp = self
            .http
            .post(&self.cfg.token_url)
            .json(&serde_json::json!({
                "code": code,
                "code_verifier": verifier,
                "code_challenge_method": Pkce::METHOD,
            }))
            .send()
            .await
            .context("key exchange request failed")?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "key exchange returned {status}: {}",
                body.chars().take(400).collect::<String>()
            );
        }

        #[derive(Deserialize)]
        struct KeyResponse {
            key: String,
        }

        let kr: KeyResponse = serde_json::from_str(&body).context(
            "the key exchange succeeded but returned no `key` field — \
             the authorization code may have already been used (they are \
             single-use and expire after ten minutes)",
        )?;

        Ok(Credential {
            access_token: kr.key,
            refresh_token: None,
            expires_at: None,
            extra: Default::default(),
        })
    }

    async fn post_token(&self, params: &[(&str, &str)]) -> Result<Credential> {
        let req = self.http.post(&self.cfg.token_url);
        let req = match self.cfg.token_body {
            TokenBody::Form => req.form(params),
            TokenBody::Json => req.json(
                &params
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), serde_json::Value::String((*v).to_string())))
                    .collect::<serde_json::Map<String, serde_json::Value>>(),
            ),
        };

        let resp = req
            .send()
            .await
            .context("token endpoint request failed")?;

        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            bail!(
                "token endpoint returned {status}: {}",
                body.chars().take(400).collect::<String>()
            );
        }

        let tr: TokenResponse = serde_json::from_str(&body)
            .with_context(|| format!("could not parse token response: {}", &body[..body.len().min(200)]))?;

        let mut extra = std::collections::HashMap::new();

        // Anthropic returns the plan alongside the token, and it is available
        // nowhere else in own-grant mode — the usage endpoint omits it.
        if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&body) {
            for key in ["subscription_type", "subscriptionType"] {
                if let Some(v) = raw.get(key).and_then(|v| v.as_str()) {
                    extra.insert("subscription_type".to_string(), v.to_string());
                    break;
                }
            }
        }

        if let Some(id_token) = tr.id_token.as_deref() {
            // Codex carries the ChatGPT account id in the id_token, and the
            // usage endpoint requires it as a header.
            if let Some(acc) = account_id_from_id_token(id_token) {
                extra.insert("account_id".to_string(), acc);
            }
        }

        Ok(Credential {
            access_token: tr.access_token,
            refresh_token: tr.refresh_token,
            expires_at: tr
                .expires_in
                .map(|s| Utc::now() + ChronoDuration::seconds(s)),
            extra,
        })
    }
}

/// Claude's hosted callback shows the code as `code#state`, and users paste it
/// with stray whitespace as often as not.
fn split_pasted_code(pasted: &str) -> (String, Option<String>) {
    let t = pasted.trim();
    match t.split_once('#') {
        Some((code, state)) => (code.trim().to_string(), Some(state.trim().to_string())),
        None => (t.to_string(), None),
    }
}

/// Pull the ChatGPT account id out of an OIDC id_token.
///
/// This reads the payload without verifying the signature, which is correct
/// here: the token arrived over TLS directly from the token endpoint, and we
/// use the claim only to address a later request — never to authorize anything.
pub fn account_id_from_id_token(id_token: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};

    let payload = id_token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;

    // The claim has moved around between Codex releases; accept either shape.
    claims
        .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")
        .and_then(|v| v.as_str())
        .or_else(|| claims.get("chatgpt_account_id").and_then(|v| v.as_str()))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OAuthConfig {
        OAuthConfig {
            flow: Flow::Oauth2,
            authorize_url: "https://example.com/oauth/authorize".into(),
            token_url: "https://example.com/oauth/token".into(),
            client_id: "https://example.com/metadata".into(),
            scopes: vec!["a:b".into(), "c:d".into()],
            redirect: RedirectMode::Loopback {
                port: 0,
                path: "/callback".into(),
                allow_paste: false,
            },
            extra_authorize_params: vec![("flow".into(), "simple".into())],
            token_body: TokenBody::Form,
            exchange_echoes_state: false,
            refresh_scopes: Vec::new(),
        }
    }

    #[test]
    fn authorize_url_carries_pkce_and_state() {
        let c = OAuthClient::new(cfg());
        let pkce = Pkce::generate();
        let url = c
            .authorize_url("http://127.0.0.1:9999/callback", &pkce.challenge, "st4te")
            .unwrap();

        assert!(url.contains("response_type=code"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", pkce.challenge)));
        assert!(url.contains("state=st4te"));
        assert!(url.contains("flow=simple"));
        // A client_id that is itself a URL must survive encoding intact.
        assert!(url.contains("client_id=https%3A%2F%2Fexample.com%2Fmetadata"));
        // Space-separated scopes, percent-encoded.
        assert!(url.contains("scope=a%3Ab+c%3Ad"));
    }

    #[test]
    fn expiry_uses_skew() {
        let mut c = Credential {
            access_token: "t".into(),
            refresh_token: None,
            expires_at: Some(Utc::now() + ChronoDuration::seconds(30)),
            extra: Default::default(),
        };
        // 30s out is inside the 60s skew, so we treat it as already expired.
        assert!(c.is_expired());

        c.expires_at = Some(Utc::now() + ChronoDuration::seconds(600));
        assert!(!c.is_expired());

        // No expiry recorded (OpenRouter's key) means never expired.
        c.expires_at = None;
        assert!(!c.is_expired());
    }

    #[test]
    fn extracts_account_id_from_namespaced_claim() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        let claims = serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": "acc-123" }
        });
        let tok = format!(
            "hdr.{}.sig",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap())
        );
        assert_eq!(account_id_from_id_token(&tok).as_deref(), Some("acc-123"));
    }

    #[test]
    fn splits_hosted_callback_code_and_state() {
        // Claude's hosted page shows `code#state`.
        let (c, s) = split_pasted_code("abc123#st4te");
        assert_eq!(c, "abc123");
        assert_eq!(s.as_deref(), Some("st4te"));

        // Users paste with stray whitespace and newlines constantly.
        let (c, s) = split_pasted_code("  abc123#st4te \n");
        assert_eq!(c, "abc123");
        assert_eq!(s.as_deref(), Some("st4te"));

        // Code alone is valid; there is simply no state to verify.
        let (c, s) = split_pasted_code("abc123\n");
        assert_eq!(c, "abc123");
        assert_eq!(s, None);
    }

    #[test]
    fn hosted_paste_authorize_url_uses_the_hosted_redirect() {
        let mut c = cfg();
        c.redirect = RedirectMode::HostedPaste {
            redirect_uri: "https://platform.claude.com/oauth/code/callback".into(),
        };
        let client = OAuthClient::new(c);
        let url = client
            .authorize_url(
                "https://platform.claude.com/oauth/code/callback",
                &Pkce::generate().challenge,
                "s",
            )
            .unwrap();
        assert!(url.contains("redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback"));
    }

    #[test]
    fn openrouter_authorize_url_carries_only_what_that_flow_defines() {
        let mut c = cfg();
        c.flow = Flow::OpenRouterKey;
        c.authorize_url = "https://openrouter.ai/auth".into();
        c.extra_authorize_params.clear();
        let client = OAuthClient::new(c);
        let url = client
            .authorize_url("http://localhost:41234/callback", "chal", "st4te")
            .unwrap();

        assert!(url.contains("callback_url=http%3A%2F%2Flocalhost%3A41234%2Fcallback"));
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        // No client to identify, no scope to request, and no `state` — sending
        // any of them would be inventing parameters this endpoint never defined.
        assert!(!url.contains("client_id="), "{url}");
        assert!(!url.contains("scope="), "{url}");
        assert!(!url.contains("state="), "{url}");
        assert!(!url.contains("redirect_uri="), "{url}");
    }

    #[tokio::test]
    async fn a_key_exchange_flow_refuses_to_refresh() {
        let mut c = cfg();
        c.flow = Flow::OpenRouterKey;
        let err = OAuthClient::new(c).refresh("rt").await.unwrap_err().to_string();
        assert!(err.contains("nothing to refresh"), "{err}");
    }

    #[test]
    fn malformed_id_token_is_none_not_panic() {
        assert!(account_id_from_id_token("not-a-jwt").is_none());
        assert!(account_id_from_id_token("a.!!!.c").is_none());
    }
}
