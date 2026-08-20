//! What a provider tells the UI about itself.
//!
//! This is the plugin interface. The config screen renders entirely from these
//! structures — which providers exist, how each can be signed in to, what to
//! call the field it wants pasted — so adding a provider means writing one
//! adapter and nothing else. No Vue component, no daemon route and no `match`
//! anywhere downstream knows that "openrouter" is a word.
//!
//! Deliberately *derived* rather than hand-declared. Whether a provider offers
//! a browser login is not a boolean an adapter sets: it is whether
//! [`Adapter::oauth_config`](super::Adapter::oauth_config) succeeds. Whether it
//! can borrow a CLI's token is whether it names a path. A separate declaration
//! would be a second source of truth, and the two would drift the first time
//! somebody added a flow without updating the list.

use serde::Serialize;
use std::path::PathBuf;
use ts_rs::TS;

use super::AuthPreference;

/// One provider, as the "Add provider" screen sees it.
#[derive(TS, Debug, Clone, Serialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct ProviderInfo {
    pub id: String,
    pub label: String,
    /// One line, shown under the name in the picker.
    pub summary: String,
    /// The provider's own mark, as a `data:` URI.
    ///
    /// Inlined rather than linked because the panel's CSP is
    /// `img-src 'self' data:`: an `http://localhost:<port>/icon` would be
    /// blocked outright, and even allowed it would leave a row of broken
    /// squares whenever the daemon was slow or on another machine. A name is
    /// read; a mark is recognised, which is the whole point of putting one
    /// next to a list of four near-identical rows.
    pub icon: String,
    #[ts(optional)]
    pub docs_url: Option<String>,
    /// Every way in to this provider, best first. Never empty — a provider
    /// with no way to authenticate could not have an adapter.
    pub methods: Vec<AuthMethod>,
}

/// One way of authenticating, in the UI's terms rather than the protocol's.
#[derive(TS, Debug, Clone, Serialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct AuthMethod {
    pub auth: AuthPreference,
    pub kind: LoginKind,
    pub label: String,
    /// What actually happens, in one line. The user is picking between
    /// "browser" and "borrow the CLI's token" and deserves to know the
    /// difference before clicking.
    pub detail: String,
    /// The adapter's own default — rendered as "Recommended".
    pub recommended: bool,
    /// False when this machine cannot use it: no vendor CLI installed, say.
    /// Still listed, because *why* it is unavailable is the useful part.
    pub available: bool,
    #[ts(optional)]
    pub unavailable_reason: Option<String>,
    /// Present only for [`LoginKind::Paste`].
    #[ts(optional)]
    pub token: Option<TokenPrompt>,
}

/// What the UI has to *do* for a method. The three shapes need three different
/// screens, and this is the only thing the frontend switches on.
#[derive(TS, Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../../widget/src/types/")]
pub enum LoginKind {
    /// Open a browser and wait. The daemon runs the PKCE exchange.
    Browser,
    /// Show a text field. The user brings a key from somewhere else.
    Paste,
    /// Nothing to do: read the vendor CLI's file and start polling.
    Borrow,
}

/// How to label and explain a pasted secret.
///
/// Every provider words this differently — one wants an API key from a web
/// console, another wants the output of a CLI command — and getting it wrong
/// sends the user hunting through the wrong settings page.
#[derive(TS, Debug, Clone, Serialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct TokenPrompt {
    /// What to call the button: "Paste an API key".
    ///
    /// Spelled out per provider rather than composed from `label`. Building it
    /// produced "Paste an api key" — English articles and capitalisation do not
    /// survive `to_lowercase`, and a wrong-sounding button is exactly the kind
    /// of thing that makes a setup screen feel unfinished.
    pub action: String,
    /// Field label: "API key", "Token".
    pub label: String,
    pub placeholder: String,
    /// One line on where to get it.
    pub help: String,
    /// A page to open, when there is one to open.
    #[ts(optional)]
    pub url: Option<String>,
}

/// The part of [`ProviderInfo`] an adapter actually writes.
///
/// Everything else — which methods exist, whether they work here — is worked
/// out from the adapter's other methods by [`super::Any::info`].
#[derive(Debug, Clone)]
pub struct Spec {
    pub summary: &'static str,
    /// The provider's mark as PNG bytes, `include_bytes!` from `icons/`.
    ///
    /// Compiled in rather than fetched: these are 64x64 and palette-quantised
    /// precisely because all of them ride along in every `/providers`
    /// response, and downloading a logo at runtime would make the config
    /// screen depend on the network to render a list of things it already
    /// knows.
    pub icon: &'static [u8],
    pub docs_url: Option<&'static str>,
    /// `None` for providers where a pasted key cannot work. Codex is the case
    /// that matters: its usage endpoint needs an OAuth access token and an
    /// account id from the id_token, so an API key would be accepted by the
    /// field and then fail on every poll.
    pub token: Option<TokenPrompt>,
    /// Which vendor CLI a delegated read borrows from, for the UI to name.
    pub vendor_cli: Option<&'static str>,
}

impl Spec {
    /// A spec with only the two fields every provider must answer.
    pub fn new(summary: &'static str, icon: &'static [u8]) -> Self {
        Spec {
            summary,
            icon,
            docs_url: None,
            token: None,
            vendor_cli: None,
        }
    }

    pub fn docs(mut self, url: &'static str) -> Self {
        self.docs_url = Some(url);
        self
    }

    pub fn token(
        mut self,
        action: &str,
        label: &str,
        placeholder: &str,
        help: &str,
        url: Option<&str>,
    ) -> Self {
        self.token = Some(TokenPrompt {
            action: action.into(),
            label: label.into(),
            placeholder: placeholder.into(),
            help: help.into(),
            url: url.map(Into::into),
        });
        self
    }

    pub fn vendor_cli(mut self, name: &'static str) -> Self {
        self.vendor_cli = Some(name);
        self
    }
}

/// Wrap PNG bytes as a `data:` URI an `<img src>` can take verbatim.
fn data_uri(png: &[u8]) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine};
    format!("data:image/png;base64,{}", STANDARD.encode(png))
}

/// Assemble the manifest from what the adapter can answer.
///
/// `oauth` and `delegated` are passed in rather than looked up because the
/// caller already holds the adapter and this file must not know the enum.
pub(super) fn build(
    id: &str,
    label: &str,
    spec: Spec,
    default_auth: AuthPreference,
    // `Some(reason)` when there is no browser flow — opencode mints its keys
    // in a web console — and the reason is what that row shows.
    oauth_error: Option<String>,
    delegated: Option<PathBuf>,
) -> ProviderInfo {
    // Browser first when it works: it is the only method that survives on a
    // machine without the vendor CLI, which is every phone and most Windows
    // installs.
    let mut methods = vec![AuthMethod {
        auth: AuthPreference::Own,
        kind: LoginKind::Browser,
        label: "Sign in with your browser".into(),
        detail: "usage-watcher gets its own credential and keeps it fresh.".into(),
        recommended: default_auth == AuthPreference::Own,
        available: oauth_error.is_none(),
        unavailable_reason: oauth_error,
        token: None,
    }];

    if let Some(path) = delegated {
        let exists = path.exists();
        let cli = spec.vendor_cli.unwrap_or("the vendor CLI");
        methods.push(AuthMethod {
            auth: AuthPreference::Delegated,
            kind: LoginKind::Borrow,
            label: format!("Use the {cli} sign-in"),
            detail: format!(
                "Reads {}, read-only. Nothing to sign in to, and \
                 usage-watcher never refreshes a borrowed token.",
                path.display()
            ),
            recommended: default_auth == AuthPreference::Delegated,
            available: exists,
            unavailable_reason: (!exists).then(|| format!("{cli} is not signed in on this machine.")),
            token: None,
        });
    }

    if let Some(prompt) = spec.token {
        methods.push(AuthMethod {
            auth: AuthPreference::Token,
            kind: LoginKind::Paste,
            label: prompt.action.clone(),
            detail: prompt.help.clone(),
            recommended: default_auth == AuthPreference::Token,
            available: true,
            unavailable_reason: None,
            token: Some(prompt),
        });
    }

    ProviderInfo {
        id: id.to_string(),
        label: label.to_string(),
        summary: spec.summary.to_string(),
        icon: data_uri(spec.icon),
        docs_url: spec.docs_url.map(Into::into),
        methods,
    }
}
