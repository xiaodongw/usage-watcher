//! Adding, removing and signing in to providers.
//!
//! The daemon used to be read-only, which meant every setup step happened in a
//! terminal: edit TOML, run `uw auth login`, restart. This module is what lets
//! the panel do it instead — and it lives on the *daemon* rather than in the
//! Tauri shell for one reason. The credentials are here. In the arrangement
//! this project was built for, the daemon runs inside WSL where the vendor CLIs
//! and their tokens are, and the widget runs on Windows; a login driven from
//! the widget's own process would store the token on the wrong side of that
//! line. Everything below therefore works identically whether the viewer is
//! the embedded webview, a browser, or a phone on the same Tailnet.
//!
//! Nothing here reads or writes a secret in a response body. A credential goes
//! in through [`set_token`] and is never handed back out; the rest of the API
//! deals in "signed in: yes/no".

use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uw_core::auth::{Credential, RedirectMode, TokenStore};
use uw_core::providers::{Any, AuthPreference, ProviderInfo};
use uw_core::Config;

use crate::hub::Event;
use crate::http::{authorize, Auth};
use crate::login::{self, Phase};
use crate::AppState;

/// Everything the config screen renders from.
#[derive(TS, Debug, Clone, Serialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct ProvidersView {
    /// Every provider that *could* be added, with its login methods. Built
    /// from the adapters themselves, so a new provider appears here — and
    /// therefore in the UI — without a line of frontend changing.
    pub catalogue: Vec<ProviderInfo>,
    /// What the user has actually added, in the order they are displayed.
    pub configured: Vec<ConfiguredProvider>,
}

#[derive(TS, Debug, Clone, Serialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct ConfiguredProvider {
    pub id: String,
    pub label: String,
    pub accent: String,
    pub auth: AuthPreference,
    pub enabled: bool,
    /// Whether a credential exists — not whether it still works. The provider
    /// tile answers the second question; this one decides whether the row
    /// offers "Sign in" or "Signed in".
    pub signed_in: bool,
    /// Why it is not signed in, when there is something useful to say.
    #[ts(optional)]
    pub note: Option<String>,
}

/// What a started login tells the client.
#[derive(TS, Debug, Clone, Serialize)]
#[ts(export, export_to = "../../../widget/src/types/")]
pub struct LoginStarted {
    /// Echoed back when posting the code, so a stale tab cannot finish someone
    /// else's login.
    pub session: String,
    pub phase: Phase,
}

#[derive(Debug, Deserialize)]
pub struct AddRequest {
    pub id: String,
    /// Which of the provider's methods the user picked.
    pub auth: AuthPreference,
}

#[derive(Debug, Deserialize)]
pub struct CodeRequest {
    pub session: String,
    pub code: String,
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub token: String,
}

// ------------------------------------------------------------------ handlers

pub async fn list(State(st): State<AppState>, headers: HeaderMap, Query(q): Query<Auth>) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    match view(&st).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Add a provider, or change how an existing one authenticates.
///
/// Does not sign in. The client follows up with [`start_login`] when the method
/// it chose is a browser flow — two calls rather than one because a failed
/// login must still leave the provider in the list, visible and removable,
/// rather than rolling back to a screen with no trace of what went wrong.
pub async fn add(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Auth>,
    Json(body): Json<AddRequest>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    let Some(adapter) = Any::by_id(&body.id) else {
        return unknown(&body.id);
    };

    // Validated before the write, never after. A mode this provider cannot
    // support would otherwise be saved and only surface later as an error tile
    // with no hint of which click caused it.
    if let Err(e) = adapter.auth_mode(body.auth) {
        return fail(StatusCode::BAD_REQUEST, e);
    }

    {
        let mut cfg = st.config.write().await;
        cfg.add(&body.id, body.auth);
        if let Err(e) = cfg.save() {
            return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    resync(&st).await;
    respond_with_view(&st).await
}

/// Remove a provider: config entry, stored credentials, and its tile.
pub async fn remove(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Auth>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    let Some(adapter) = Any::by_id(&id) else {
        return unknown(&id);
    };

    {
        let mut cfg = st.config.write().await;
        cfg.remove(&id);
        if let Err(e) = cfg.save() {
            return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }

    // Config first, credentials second. If the process dies between the two we
    // are left with an orphaned keychain entry, which is inert; the other order
    // would leave a provider still being polled with its token deleted, which
    // is an error tile the user cannot clear.
    if let Err(e) = adapter.forget_credentials() {
        tracing::warn!(provider = %id, "removed, but could not delete the credential: {e:#}");
    }

    // Stops the poller and takes the tile off every connected panel.
    resync(&st).await;
    respond_with_view(&st).await
}

/// Begin a browser login and hand back the URL for the viewer to open.
pub async fn start_login(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Auth>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    match begin_login(&st, &id).await {
        Ok(started) => Json(started).into_response(),
        Err(e) => fail(StatusCode::BAD_REQUEST, e),
    }
}

/// Where a login has got to. Polled by clients that cannot hold the SSE stream.
pub async fn login_status(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Auth>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    match st.logins.get(&id).await {
        Some(s) => Json(LoginStarted {
            session: s.id.clone(),
            phase: s.phase(),
        })
        .into_response(),
        None => fail_msg(StatusCode::NOT_FOUND, "no sign-in is in progress"),
    }
}

/// Deliver the code from a provider that shows one instead of redirecting.
pub async fn submit_code(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Auth>,
    Json(body): Json<CodeRequest>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    let Some(session) = st.logins.get(&id).await else {
        return fail_msg(StatusCode::NOT_FOUND, "no sign-in is in progress");
    };
    if session.id != body.session {
        return fail_msg(
            StatusCode::CONFLICT,
            "that code belongs to a sign-in attempt that has been superseded",
        );
    }
    let code = body.code.trim().to_string();
    if code.is_empty() {
        return fail_msg(StatusCode::BAD_REQUEST, "no code given");
    }
    match session.submit_code(code) {
        Ok(()) => Json(LoginStarted {
            session: session.id.clone(),
            phase: session.phase(),
        })
        .into_response(),
        Err(e) => fail(StatusCode::CONFLICT, e),
    }
}

/// Store a pasted API key and switch the provider to it.
pub async fn set_token(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Auth>,
    Json(body): Json<TokenRequest>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    let Some(adapter) = Any::by_id(&id) else {
        return unknown(&id);
    };
    let token = body.token.trim().to_string();
    if token.is_empty() {
        return fail_msg(StatusCode::BAD_REQUEST, "no token given");
    }

    // No expiry and no refresh token: these are long-lived by design, so
    // recording an expiry we invented would only make the poller give up early.
    let cred = Credential {
        access_token: token,
        refresh_token: None,
        expires_at: None,
        extra: Default::default(),
    };
    if let Err(e) = TokenStore::save(&adapter.token_entry(), &cred) {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
    }

    {
        let mut cfg = st.config.write().await;
        cfg.add(&id, AuthPreference::Token);
        if let Err(e) = cfg.save() {
            return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
        }
    }
    // Restart rather than sync: the config may be unchanged from the
    // supervisor's point of view, but the credential behind it is new and the
    // running task is sitting on a backoff holding an error that no longer
    // applies.
    restart(&st, &id).await;
    respond_with_view(&st).await
}

/// Forget a provider's credential but keep the provider.
pub async fn logout(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(q): Query<Auth>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    let Some(adapter) = Any::by_id(&id) else {
        return unknown(&id);
    };
    if let Err(e) = adapter.forget_credentials() {
        return fail(StatusCode::INTERNAL_SERVER_ERROR, e);
    }
    restart(&st, &id).await;
    respond_with_view(&st).await
}

// ------------------------------------------------------------------ internals

async fn begin_login(st: &AppState, id: &str) -> Result<LoginStarted> {
    let adapter = Any::by_id(id).with_context(|| format!("unknown provider `{id}`"))?;
    let oauth = adapter.oauth_config()?;

    // Whether the UI should offer a code field, taken from the flow rather than
    // guessed. Two shapes need one: a provider that only ever displays a code,
    // and one that redirects but *can* display a code when the browser cannot
    // reach our listener. The second is Claude, and it is not a rare fallback —
    // it is what happens every time the daemon is in WSL and the browser is on
    // Windows, which is the arrangement this project was built for.
    let needs_code = match &oauth.redirect {
        RedirectMode::HostedPaste { .. } => true,
        RedirectMode::Loopback { allow_paste, .. } => *allow_paste,
    };

    // Signing in only makes sense for an own grant, so flip the toggle rather
    // than failing with "this provider is in delegated mode" — same as the CLI.
    {
        let mut cfg = st.config.write().await;
        cfg.add(id, AuthPreference::Own);
        cfg.save()?;
    }

    let source = adapter.token_source(AuthPreference::Own)?;
    let (session, ui) = login::session(id, needs_code);

    let task = tokio::spawn({
        let st = st.clone();
        let session = session.clone();
        let id = id.to_string();
        async move {
            let outcome = async {
                let mut cred = source.login(&ui).await?;
                // Best effort: this only decorates the tile with a plan name,
                // and the credential is already stored and usable. Refusing a
                // good token over a missing label would be far worse.
                match uw_core::providers::enrich(&id, &st.http, &mut cred).await {
                    Ok(()) => source.store(cred).await?,
                    Err(e) => tracing::warn!(
                        provider = %id,
                        "signed in, but could not read the account profile: {e:#}"
                    ),
                }
                anyhow::Ok(())
            }
            .await;

            session.set(match outcome {
                Ok(()) => Phase::Done,
                Err(e) => Phase::Failed {
                    message: format!("{e:#}"),
                },
            });

            // Either way the provider's state changed: on success there is a
            // credential to poll with, on failure an error tile to refresh.
            restart(&st, &id).await;
        }
    });

    st.logins.install(session.clone(), task).await;

    // Wait for the OAuth client to produce the URL, rather than answering
    // "Opening" and making the client poll for the one thing it needs. Building
    // it is pure computation, so this returns almost immediately; the timeout
    // is only here so a wedged login cannot hold an HTTP request open.
    let phase = tokio::time::timeout(std::time::Duration::from_secs(15), session.opened())
        .await
        .unwrap_or(Phase::Failed {
            message: "timed out building the authorization URL".into(),
        });

    Ok(LoginStarted {
        session: session.id.clone(),
        phase,
    })
}

/// Reconcile the pollers with config, then tell every viewer.
async fn resync(st: &AppState) {
    let cfg = st.config.read().await.clone();
    st.supervisor.sync(&cfg).await;
    broadcast(st).await;
}

/// Restart one provider's poller, then tell every viewer.
async fn restart(st: &AppState, id: &str) {
    let cfg = st.config.read().await.clone();
    st.supervisor.restart(id, &cfg).await;
    broadcast(st).await;
}

/// Push the provider list to connected viewers.
///
/// The reason a config screen updates the instant a browser login completes:
/// that happens on a spawned task, long after the request that started it was
/// answered, so there is no response left to put it in.
pub async fn broadcast(st: &AppState) {
    match view(st).await {
        Ok(v) => st.hub.publish(Event::Providers(v)),
        Err(e) => tracing::warn!("could not build the provider view: {e:#}"),
    }
}

async fn respond_with_view(st: &AppState) -> Response {
    match view(st).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

/// Build the whole config-screen payload.
///
/// On a blocking thread: it stats the vendor CLIs' credential files and asks
/// the OS keychain whether each entry exists. Neither is slow, but both are
/// synchronous I/O, and on Windows the credential manager is a real syscall —
/// not something to run on the reactor once per config-screen render.
async fn view(st: &AppState) -> Result<ProvidersView> {
    let cfg = st.config.read().await.clone();
    tokio::task::spawn_blocking(move || build_view(&cfg))
        .await
        .context("the provider view task panicked")?
}

fn build_view(cfg: &Config) -> Result<ProvidersView> {
    let catalogue = Any::catalogue();

    let configured = cfg
        .providers
        .iter()
        .filter_map(|(id, pc)| {
            // An id with no adapter is not an error worth failing the whole
            // screen over: it is what a config file from a newer build, or a
            // typo, looks like. Skipping it keeps the rest usable.
            let adapter = Any::by_id(id)?;
            let (signed_in, note) = match adapter.has_credential(pc.auth) {
                Ok(true) => (true, None),
                Ok(false) => (false, None),
                // The mode itself does not work here — delegated with no CLI
                // installed, say. That is the useful thing to show.
                Err(e) => (false, Some(format!("{e:#}"))),
            };
            let info = adapter.info();
            Some(ConfiguredProvider {
                id: id.clone(),
                label: info.label,
                accent: info.accent,
                auth: pc.auth,
                enabled: pc.enabled,
                signed_in,
                note,
            })
        })
        .collect();

    Ok(ProvidersView {
        catalogue,
        configured,
    })
}

// ------------------------------------------------------------------- errors

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

fn fail(status: StatusCode, e: anyhow::Error) -> Response {
    fail_msg(status, &format!("{e:#}"))
}

fn fail_msg(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ErrorBody {
            error: message.to_string(),
        }),
    )
        .into_response()
}

fn unknown(id: &str) -> Response {
    fail_msg(
        StatusCode::NOT_FOUND,
        &format!("unknown provider `{id}` (known: {})", Any::known_ids()),
    )
}
