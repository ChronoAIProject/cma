use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use cma_sdk::{Client, Error};
use serde_json::json;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

struct Server {
    base_url: String,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve(router: Router) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        base_url: format!("http://{address}"),
        task,
    }
}

#[tokio::test]
async fn reads_the_anonymous_discovery_document() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|headers: HeaderMap| async move {
            assert!(headers.get(header::AUTHORIZATION).is_none());
            axum::Json(json!({
                "service": "cma",
                "status": "rebuilding",
                "interfaces": [],
            }))
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url).unwrap().build().unwrap();

    let document = client.read_conformance().await.unwrap();

    assert_eq!(document.service, "cma");
    assert_eq!(document.status, "rebuilding");
    assert!(document.interfaces.is_empty());
    assert!(format!("{client:?}").contains("base_url"));
}

#[tokio::test]
async fn preserves_a_reverse_proxy_prefix() {
    let router = Router::new().route(
        "/gateway/api/v1/conformance",
        get(|| async {
            axum::Json(json!({
                "service": "cma",
                "status": "ready",
                "interfaces": ["rest"],
            }))
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(format!("{}/gateway", server.base_url))
        .unwrap()
        .build()
        .unwrap();

    let document = client.read_conformance().await.unwrap();

    assert_eq!(document.interfaces, ["rest"]);
}

#[tokio::test]
async fn decodes_generic_problem_documents() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|| async {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                [
                    (header::CONTENT_TYPE, "application/problem+json"),
                    (
                        header::HeaderName::from_static("x-request-id"),
                        "request-801",
                    ),
                ],
                json!({
                    "status": 503,
                    "detail": "deployment is unavailable",
                    "code": "unavailable",
                })
                .to_string(),
            )
                .into_response()
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url).unwrap().build().unwrap();

    let error = client.read_conformance().await.unwrap_err();

    assert_eq!(
        error,
        Error::Api {
            status: 503,
            code: Some("unavailable".into()),
            message: "deployment is unavailable".into(),
            request_id: Some("request-801".into()),
        }
    );
}

#[tokio::test]
async fn rejects_invalid_success_and_failure_documents() {
    for (status, body) in [
        (StatusCode::OK, json!({"service": "cma", "status": ""})),
        (
            StatusCode::OK,
            json!({
                "service": "cma",
                "status": "rebuilding",
                "interfaces": [],
                "extra": true,
            }),
        ),
        (
            StatusCode::BAD_GATEWAY,
            json!({"status": 503, "detail": "mismatch"}),
        ),
        (StatusCode::BAD_GATEWAY, json!({"code": "missing_detail"})),
    ] {
        let router = Router::new().route(
            "/api/v1/conformance",
            get(move || async move { (status, axum::Json(body)) }),
        );
        let server = serve(router).await;
        let client = Client::builder(&server.base_url).unwrap().build().unwrap();
        assert!(matches!(
            client.read_conformance().await,
            Err(Error::Protocol { .. })
        ));
    }
}

#[tokio::test]
async fn rejects_non_json_problem_responses() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|| async { (StatusCode::BAD_GATEWAY, "not json") }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url).unwrap().build().unwrap();

    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Protocol { .. })
    ));
}

#[tokio::test]
async fn applies_the_configured_deadline() {
    let router = Router::new().route(
        "/api/v1/conformance",
        get(|| async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            axum::Json(json!({
                "service": "cma",
                "status": "rebuilding",
                "interfaces": [],
            }))
        }),
    );
    let server = serve(router).await;
    let client = Client::builder(&server.base_url)
        .unwrap()
        .request_timeout(Duration::from_millis(5))
        .unwrap()
        .build()
        .unwrap();

    assert!(matches!(
        client.read_conformance().await,
        Err(Error::Transport { .. })
    ));
}

#[test]
fn validates_client_configuration() {
    for invalid in [
        "not a URL",
        "ftp://cma.example",
        "https://user@cma.example",
        "https://cma.example?mode=test",
        "https://cma.example/#fragment",
    ] {
        assert!(matches!(
            Client::builder(invalid),
            Err(Error::Protocol { .. })
        ));
    }

    let builder = Client::builder("https://cma.example").unwrap();
    assert!(format!("{builder:?}").contains("base_url"));
    assert!(matches!(
        builder.request_timeout(Duration::ZERO),
        Err(Error::Protocol { .. })
    ));
}
