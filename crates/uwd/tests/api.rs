//! The API contract viewers depend on: what is served, and what is refused.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::sync::RwLock;
use tower::ServiceExt;
use uwd::hub::Hub;
use uwd::supervisor::Supervisor;
use uwd::AppState;

fn app(token: Option<&str>) -> axum::Router {
    let hub = Arc::new(Hub::new(10));
    let http = uw_core::http_client();
    uwd::http::router(AppState {
        supervisor: Arc::new(Supervisor::new(hub.clone(), http.clone())),
        hub,
        http,
        logins: Arc::new(uwd::login::Logins::default()),
        // Never `Config::load()`: a test must not read, let alone migrate, the
        // config of whoever is running it.
        config: Arc::new(RwLock::new(uw_core::Config::fresh())),
        token: token.map(str::to_string),
    })
}

async fn send(app: axum::Router, req: Request<Body>) -> (StatusCode, String) {
    let res = app.oneshot(req).await.unwrap();
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 64 * 1024).await.unwrap();
    (status, String::from_utf8_lossy(&body).into_owned())
}

async fn status_of(app: axum::Router, uri: &str, bearer: Option<&str>) -> StatusCode {
    let mut req = Request::builder().uri(uri);
    if let Some(t) = bearer {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    send(app, req.body(Body::empty()).unwrap()).await.0
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

// ------------------------------------------------------- the provider API

#[tokio::test]
async fn the_catalogue_lists_every_provider_with_a_way_in() {
    let (status, body) = send(
        app(None),
        Request::builder()
            .uri("/providers")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let catalogue = v["catalogue"].as_array().unwrap();
    assert_eq!(catalogue.len(), uw_core::providers::Any::all().len());

    for p in catalogue {
        let methods = p["methods"].as_array().unwrap();
        assert!(
            methods.iter().any(|m| m["available"] == true),
            "{} offers no usable method at all",
            p["id"]
        );
    }
    // A fresh config has added nothing, which is what puts the panel on its
    // "Add provider" empty state.
    assert!(v["configured"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn writing_routes_are_behind_the_token_too() {
    // These mint and delete credentials. Leaving them open while the read
    // routes were gated would be the whole point of the token missed.
    let a = app(Some("s3cret"));
    let (status, _) = send(
        a,
        Request::builder()
            .method("POST")
            .uri("/providers")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"id":"claude","auth":"own"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    assert_eq!(
        status_of(app(Some("s3cret")), "/providers", None).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_unknown_provider_is_refused_before_anything_is_written() {
    let (status, body) = send(
        app(None),
        Request::builder()
            .method("POST")
            .uri("/providers")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"id":"not-a-provider","auth":"own"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body.contains("known:"), "{body}");
}

#[tokio::test]
async fn asking_about_a_login_nobody_started_is_a_404_not_a_hang() {
    assert_eq!(
        status_of(app(None), "/providers/claude/login", None).await,
        StatusCode::NOT_FOUND
    );
}

// ------------------------------------------------------------------ CORS

async fn preflight(origin: &str, method: &str) -> Option<String> {
    let res = app(None)
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri("/providers")
                .header("origin", origin)
                .header("access-control-request-method", method)
                .header("access-control-request-headers", "content-type")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    res.headers()
        .get("access-control-allow-origin")
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn our_own_viewers_may_call_the_write_api() {
    for origin in [
        "tauri://localhost",
        "https://tauri.localhost",
        "http://localhost:5173",
        "http://127.0.0.1:1420",
    ] {
        assert_eq!(
            preflight(origin, "POST").await.as_deref(),
            Some(origin),
            "{origin} was refused"
        );
    }
}

#[tokio::test]
async fn a_web_page_cannot_reach_the_write_api() {
    // The attack this closes: any site you visit can talk to your loopback
    // ports. Reading a snapshot that way is harmless; deleting a provider or
    // starting a sign-in is not.
    for origin in [
        "https://example.com",
        "http://evil.test",
        "https://localhost.evil.com",
        "null",
    ] {
        assert_eq!(
            preflight(origin, "POST").await,
            None,
            "{origin} was granted a preflight"
        );
    }
}

#[tokio::test]
async fn reading_a_snapshot_stays_open_to_anyone() {
    // Deliberately unchanged: a GET of data that holds no secrets, so a
    // scratch HTML file can chart your usage.
    let res = app(None)
        .oneshot(
            Request::builder()
                .uri("/snapshot")
                .header("origin", "https://example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap()),
        Some("*")
    );
}
