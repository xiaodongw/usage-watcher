//! The API contract viewers depend on: what is served, and what is refused.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

// The daemon is a binary, so its modules are reached through the test harness's
// own `mod` declarations rather than a library import.
#[path = "../src/hub.rs"]
mod hub;
#[path = "../src/http.rs"]
mod http;

use hub::Hub;

fn app(token: Option<&str>) -> axum::Router {
    http::router(http::AppState {
        hub: Arc::new(Hub::new(10)),
        token: token.map(str::to_string),
    })
}

async fn status_of(app: axum::Router, uri: &str, bearer: Option<&str>) -> StatusCode {
    let mut req = Request::builder().uri(uri);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    app.oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn without_a_token_configured_everything_is_open() {
    // The default bind is loopback, where anything able to connect is already
    // running as this user.
    assert_eq!(status_of(app(None), "/snapshot", None).await, StatusCode::OK);
    assert_eq!(status_of(app(None), "/history", None).await, StatusCode::OK);
}

#[tokio::test]
async fn a_configured_token_is_required() {
    assert_eq!(
        status_of(app(Some("s3cret")), "/snapshot", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(app(Some("s3cret")), "/snapshot", Some("wrong")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        status_of(app(Some("s3cret")), "/snapshot", Some("s3cret")).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn every_data_route_is_behind_the_token_not_just_snapshot() {
    for route in ["/snapshot", "/history", "/events"] {
        assert_eq!(
            status_of(app(Some("s3cret")), route, None).await,
            StatusCode::UNAUTHORIZED,
            "{route} was reachable without the token"
        );
    }
}

#[tokio::test]
async fn health_stays_open_so_a_supervisor_can_probe_it() {
    assert_eq!(
        status_of(app(Some("s3cret")), "/health", None).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn the_token_is_also_accepted_as_a_query_parameter() {
    // EventSource cannot set an Authorization header, so without this the SSE
    // stream would be unreachable from a browser whenever a token is set.
    assert_eq!(
        status_of(app(Some("s3cret")), "/events?token=s3cret", None).await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(app(Some("s3cret")), "/events?token=nope", None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn history_still_parses_its_own_query_alongside_a_token() {
    assert_eq!(
        status_of(
            app(Some("s3cret")),
            "/history?since=2026-01-01T00:00:00Z&token=s3cret",
            None
        )
        .await,
        StatusCode::OK
    );
}
