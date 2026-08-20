//! The HTTP surface: reading in this file, writing in [`crate::api`].
//!
//! Push, not poll. A viewer fetches `/snapshot` once for immediate paint and
//! then holds `/events` open — so the widget never has its own timer, and the
//! poll rhythm is decided in exactly one place.
//!
//! ## Two CORS policies, on purpose
//!
//! The read routes stay open to any origin: they are GETs, the payload holds no
//! secrets, and letting a scratch HTML file chart your usage is a feature.
//!
//! The provider routes cannot be. They add credentials, delete them, and start
//! browser logins — so a page on some unrelated site must not be able to reach
//! them just because your daemon is on loopback. They are therefore restricted
//! to origins that are plausibly this app: a Tauri webview, or something served
//! from localhost. Two things enforce it. The browser will not send a
//! cross-origin `DELETE`, or a `POST` with a JSON content type, without a
//! successful preflight — and these routes only grant one to those origins. And
//! [`authorize`] still applies, which means anything not on loopback needs the
//! bearer token as well, because `uwd` refuses to bind a public address without
//! one in the first place.

use std::convert::Infallible;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tokio_stream::{Stream, StreamExt};

use crate::api;
use crate::hub::Event;
use crate::AppState;

pub fn router(state: AppState) -> Router {
    // A Tauri webview is `tauri://localhost` (`https://tauri.localhost` on
    // Windows) and the Vite dev server is `http://localhost:5173`; both are
    // cross-origin to the daemon, so without a CORS grant every viewer would be
    // blocked by the browser before a request was even sent.
    let read = Router::new()
        .route("/snapshot", get(snapshot))
        .route("/events", get(events))
        .route("/history", get(history))
        .route("/health", get(health))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET])
                .allow_headers([axum::http::header::AUTHORIZATION]),
        );

    let write = Router::new()
        .route("/providers", get(api::list).post(api::add))
        .route("/providers/{id}", delete(api::remove))
        .route(
            "/providers/{id}/login",
            get(api::login_status).post(api::start_login),
        )
        .route("/providers/{id}/login/code", post(api::submit_code))
        .route("/providers/{id}/token", post(api::set_token))
        .route("/providers/{id}/logout", post(api::logout))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(|origin, _| is_app_origin(origin)))
                .allow_methods([Method::GET, Method::POST, Method::DELETE])
                .allow_headers([
                    axum::http::header::AUTHORIZATION,
                    axum::http::header::CONTENT_TYPE,
                ]),
        );

    read.merge(write).with_state(state)
}

/// Is this origin plausibly one of our own viewers?
///
/// Tauri's webview, or anything served from the loopback interface — a Vite dev
/// server, or a hand-written page the user is testing against. Deliberately not
/// port-specific: dev servers move, and pinning 5173 would break the first time
/// someone had two projects open. Deliberately *not* `Any`, because these
/// routes mint and delete credentials, and "the user visited a web page" must
/// never be enough to reach them.
fn is_app_origin(origin: &axum::http::HeaderValue) -> bool {
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    // Linux and macOS; Windows uses the https form because the WebView2 custom
    // scheme handler is registered there instead.
    if origin == "tauri://localhost" || origin == "https://tauri.localhost" {
        return true;
    }
    let Ok(url) = url::Url::parse(origin) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]" | "::1"))
}

/// Unauthenticated on purpose: it reveals only that a daemon is listening,
/// which anything able to open the socket already knows.
async fn health() -> impl IntoResponse {
    Json(serde_json::json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn snapshot(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Auth>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }
    Json(st.hub.snapshot().await).into_response()
}

/// The token, when it arrives as a query parameter instead of a header.
///
/// The browser's `EventSource` cannot set request headers at all — there is no
/// options argument for them — so a header-only scheme would make `/events`
/// unreachable from the one client that matters most. Accepted on every route
/// for consistency. It is a bearer token either way; the query form is only
/// more likely to be written down somewhere, and on loopback there is no token
/// at all.
#[derive(Deserialize, Default)]
pub struct Auth {
    pub token: Option<String>,
}

#[derive(Deserialize)]
struct HistoryQuery {
    /// RFC 3339. Lets a reconnecting viewer ask only for what it missed.
    since: Option<DateTime<Utc>>,
    #[serde(flatten)]
    auth: Auth,
}

async fn history(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<HistoryQuery>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q.auth) {
        return r;
    }
    Json(st.hub.history(q.since).await).into_response()
}

async fn events(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<Auth>,
) -> Response {
    if let Err(r) = authorize(&st, &headers, &q) {
        return r;
    }

    let current = st.hub.snapshot().await;
    let live = BroadcastStream::new(st.hub.subscribe());

    // The first frame is the state as it stands, so a viewer that connects
    // between polls paints immediately instead of showing an empty panel until
    // the next tick.
    let head = tokio_stream::once(to_sse(Event::Snapshot(current)));
    let tail = live.filter_map(|r| r.ok().map(to_sse));

    let stream: Pinned = Box::pin(head.chain(tail));

    Sse::new(stream)
        // A lapsed viewer, a suspended laptop or a proxy will all quietly drop
        // an idle connection; the comment frame keeps it alive and lets the
        // browser's EventSource notice a real break and reconnect.
        .keep_alive(KeepAlive::new().interval(std::time::Duration::from_secs(15)))
        .into_response()
}

type Pinned = std::pin::Pin<Box<dyn Stream<Item = Result<SseEvent, Infallible>> + Send>>;

fn to_sse(ev: Event) -> Result<SseEvent, Infallible> {
    // Named events so the client can `addEventListener("alert", …)` rather than
    // sniffing the payload shape.
    let e = match ev {
        Event::Snapshot(s) => SseEvent::default().event("snapshot").json_data(s),
        Event::Alert(a) => SseEvent::default().event("alert").json_data(a),
        Event::Providers(p) => SseEvent::default().event("providers").json_data(p),
    };
    // Serializing our own model cannot fail; if it somehow did, an empty
    // comment frame is better than tearing down every viewer's connection.
    Ok(e.unwrap_or_else(|_| SseEvent::default().comment("unserializable")))
}

// clippy::result_large_err — the `Err` is a ready-made 401 `Response`, which is
// exactly what every caller wants to return. Boxing it to save 128 bytes on a
// path taken once per rejected request would trade clarity for nothing.
#[allow(clippy::result_large_err)]
pub fn authorize(st: &AppState, headers: &HeaderMap, query: &Auth) -> Result<(), Response> {
    let Some(expected) = &st.token else {
        return Ok(());
    };
    let given = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .or(query.token.as_deref());

    if given == Some(expected.as_str()) {
        Ok(())
    } else {
        Err((StatusCode::UNAUTHORIZED, "bearer token required").into_response())
    }
}
