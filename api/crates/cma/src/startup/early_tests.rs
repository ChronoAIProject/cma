use super::*;
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::ConnectInfo,
    http::{header, Method, Request, StatusCode},
    routing::any,
};
use http_body_util::{BodyExt as _, Empty};
use hyper_util::rt::TokioIo;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::{net::TcpStream, sync::oneshot};
use tower::ServiceExt as _;

#[tokio::test]
async fn startup_refusals_bypass_the_application_and_leave_request_bodies_unread() {
    let admission = Admission::new();
    async fn application() -> StatusCode {
        panic!("application invoked")
    }
    let app = admission.wrap(Router::new().fallback(application), None);
    for (method, path, status, body) in [
        ("GET", "/livez", StatusCode::OK, "ok"),
        ("POST", "/livez", StatusCode::SERVICE_UNAVAILABLE, "startup"),
        ("GET", "/ready", StatusCode::SERVICE_UNAVAILABLE, "startup"),
        ("GET", "/livez/", StatusCode::SERVICE_UNAVAILABLE, "startup"),
        (
            "OPTIONS",
            "/agents/1",
            StatusCode::SERVICE_UNAVAILABLE,
            "startup",
        ),
        (
            "POST",
            "/auth/callback",
            StatusCode::SERVICE_UNAVAILABLE,
            "startup",
        ),
        (
            "GET",
            "/internal/agent-manager/v1/live/1",
            StatusCode::SERVICE_UNAVAILABLE,
            "startup",
        ),
    ] {
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("origin", "https://example.invalid")
            .header("access-control-request-method", "GET")
            .body(Body::from_stream(futures_util::stream::poll_fn(
                |_| -> std::task::Poll<Option<Result<Bytes, std::io::Error>>> {
                    panic!("request body polled")
                },
            )))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), status, "{method} {path}");
        if status == StatusCode::SERVICE_UNAVAILABLE {
            assert_eq!(response.headers()[header::RETRY_AFTER], "1");
        }
        let bytes = to_bytes(response.into_body(), 128).await.unwrap();
        if body == "startup" {
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                serde_json::json!({"status":"not-ready", "reason":"startup"})
            );
        } else {
            assert_eq!(bytes, body);
        }
    }
}

#[tokio::test]
async fn same_connection_uses_current_phase_and_preserves_peer_on_both_servers() {
    for connection_age in [None, Some(Duration::from_secs(60))] {
        let admission = Admission::new();
        let app = Router::new()
            .route(
                "/ready",
                any(|| async { (StatusCode::SERVICE_UNAVAILABLE, "workers") }),
            )
            .route(
                "/agents",
                any(|ConnectInfo(peer): ConnectInfo<SocketAddr>| async move { peer.to_string() }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, stopped) = watch::channel(false);
        let server = tokio::spawn(http_server(
            listener,
            admission.wrap(app, None),
            connection_age,
            stopped,
        ));
        let conn = TcpStream::connect(address).await.unwrap();
        let peer = conn.local_addr().unwrap();
        let (mut client, connection) = hyper::client::conn::http1::handshake(TokioIo::new(conn))
            .await
            .unwrap();
        let connection = tokio::spawn(connection);
        for (path, status) in [
            ("/livez", StatusCode::OK),
            ("/ready", StatusCode::SERVICE_UNAVAILABLE),
        ] {
            let response = client
                .send_request(
                    Request::builder()
                        .method(Method::HEAD)
                        .uri(path)
                        .header(header::HOST, "localhost")
                        .body(Empty::<Bytes>::new())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            if path == "/livez" {
                assert_eq!(response.headers()[header::CONTENT_LENGTH], "2");
            }
            assert!(response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty());
        }
        for (path, status, body) in [
            ("/livez", StatusCode::OK, "ok".to_owned()),
            ("/agents", StatusCode::OK, peer.to_string()),
            (
                "/ready",
                StatusCode::SERVICE_UNAVAILABLE,
                "workers".to_owned(),
            ),
            (
                "/ready",
                StatusCode::SERVICE_UNAVAILABLE,
                "draining".to_owned(),
            ),
        ] {
            let response = client
                .send_request(
                    Request::get(path)
                        .header(header::HOST, "localhost")
                        .body(Empty::<Bytes>::new())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), status);
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            if body == "draining" {
                assert_eq!(
                    serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
                    serde_json::json!({"status":"not-ready", "reason":"draining"})
                );
            } else {
                assert_eq!(bytes, body);
            }
            if path == "/livez" {
                assert!(admission.open());
            } else if body == "workers" {
                admission.close();
                assert!(!admission.open());
            }
        }
        stop.send_replace(true);
        server.await.unwrap().unwrap();
        connection.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn drain_latches_before_late_initialization_success_and_joins_its_effect() {
    let admission = Admission::new();
    let settled = AtomicUsize::new(0);
    let (release, held) = oneshot::channel::<()>();
    let startup = async {
        held.await.unwrap();
        settled.fetch_add(1, Ordering::Relaxed);
        assert!(!admission.open());
        Ok(Initialization::Initialized)
    };
    let owner = until_stopping(
        &admission,
        startup,
        async {
            admission.stopping().await;
            Ok(())
        },
        Instant::now(),
    );
    let prestop = admission.drain();
    tokio::pin!(owner, prestop);
    assert!(futures_util::poll!(&mut owner).is_pending());
    assert!(futures_util::poll!(&mut prestop).is_pending());
    assert!(admission.is_draining());
    assert!(futures_util::poll!(&mut owner).is_pending());
    release.send(()).unwrap();
    owner.await.unwrap();
    assert_eq!(settled.load(Ordering::Relaxed), 1);
    admission.finish_drain(&Ok(()));
    prestop.await.unwrap();
}

#[tokio::test]
async fn failure_closes_admission_and_returns_the_initialization_error() {
    let admission = Admission::new();
    assert_eq!(
        until_stopping(
            &admission,
            async { Err("inventory failed".into()) },
            std::future::pending(),
            Instant::now(),
        )
        .await,
        Err("inventory failed".into())
    );
    assert!(admission.is_draining());
    assert!(!admission.open());
}

#[tokio::test]
async fn stopped_initialization_never_opens_admission() {
    let admission = Admission::new();
    let result = until_stopping(
        &admission,
        async { Ok(Initialization::Stopping) },
        std::future::pending(),
        Instant::now(),
    )
    .await;
    assert_eq!(result, Ok(()));
    assert!(!admission.serving().await);
}
