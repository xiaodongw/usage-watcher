//! The viewer-facing API: `/snapshot`, `/events`, `/history`, `/health`.
//!
//! Push, not poll. A viewer fetches `/snapshot` once for immediate paint and
//! then holds `/events` open — so the widget never has its own timer, and the
//! poll rhythm is decided in exactly one place.

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio_stream::wrappers::BroadcastStream;
use tower_http::cors::{Any, CorsLayer};
use tokio_stream::{Stream, StreamExt};

use crate::hub::{Event, Hub};

#[derive(Clone)]
pub struct AppState {
    pub hub: Arc<Hub>,
    /// When set, every route except `/health` requires it as a bearer token.
    pub token: Option<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/snapshot", get(snapshot))
        .route("/events", get(events))
        .route("/history", get(history))
        .route("/health", get(health))
        // A Tauri webview is `tauri://localhost` and the Vite dev server is
        // `http://localhost:5173`; both are cross-origin to the daemon, so
        // without this every viewer would be blocked by the browser before a
        // request was even sent. Read-only GETs, and the data itself is
        // already gated by `authorize`.
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET])
                .allow_headers([axum::http::header::AUTHORIZATION]),
        )
        .with_state(state)
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
    token: Option<String>,
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
    };
    // Serializing our own model cannot fail; if it somehow did, an empty
    // comment frame is better than tearing down every viewer's connection.
    Ok(e.unwrap_or_else(|_| SseEvent::default().comment("unserializable")))
}

fn authorize(st: &AppState, headers: &HeaderMap, query: &Auth) -> Result<(), Response> {
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
