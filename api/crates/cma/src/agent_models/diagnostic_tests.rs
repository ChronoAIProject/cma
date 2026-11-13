use super::*;
use axum::{
    body::Body,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};
use layer2_core::dependency_diagnostics::capture;

async fn fetch_response(response: axum::response::Response, reason: Option<Reason>) {
    let response = Arc::new(tokio::sync::Mutex::new(Some(response)));
    let app = Router::new().route(
        "/api/v1/proxy/s/chrono-llm-public/models",
        get(move || {
            let response = response.clone();
            async move { response.lock().await.take().unwrap() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut catalog =
        ChronoAgentModels::new(&origin, Arc::new(super::tests::NoCredentials)).unwrap();
    Arc::get_mut(&mut catalog).unwrap().http = reqwest::Client::builder()
        .timeout(Duration::from_millis(150))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let (result, failure) =
        capture(catalog.fetch("chrono-llm-public", "private-fixture-bearer")).await;
    assert_eq!(
        failure,
        reason.map(|reason| Failure::new(Dependency::ModelGateway, reason))
    );
    assert_eq!(result.is_err(), reason.is_some());
    task.abort();
}

#[tokio::test]
async fn catalogue_status_and_success_attribution() {
    for (status, reason) in [
        (502, Reason::BadGateway),
        (503, Reason::UpstreamUnavailable),
        (429, Reason::RateLimited),
        (401, Reason::UnexpectedStatus),
        (302, Reason::UnexpectedStatus),
    ] {
        fetch_response(
            (
                StatusCode::from_u16(status).unwrap(),
                "private fixture body",
            )
                .into_response(),
            Some(reason),
        )
        .await;
    }
    fetch_response(r#"{"data":[{"id":"gpt-6"}]}"#.into_response(), None).await;
}

#[tokio::test]
async fn catalogue_body_and_size_failures_are_attributed() {
    for body in ["private malformed body".to_owned(), r#"{"data":[{"id":"x"},{"id":"x"}]}"#.into(), serde_json::json!({"data": (0..=MAX_MODELS).map(|n| serde_json::json!({"id":n.to_string()})).collect::<Vec<_>>()} ).to_string()] {
        fetch_response(body.into_response(), Some(Reason::InvalidEvidence)).await;
    }
    fetch_response(
        "x".repeat(MAX_BYTES + 1).into_response(),
        Some(Reason::ResponseTooLarge),
    )
    .await;
    let stream = futures_util::stream::iter([Ok::<_, std::io::Error>("x".repeat(MAX_BYTES + 1))]);
    fetch_response(
        Body::from_stream(stream).into_response(),
        Some(Reason::ResponseTooLarge),
    )
    .await;
    let stream = futures_util::stream::once(async { Ok::<_, std::io::Error>("{") }).chain(
        futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            Ok::<_, std::io::Error>("}")
        }),
    );
    fetch_response(
        Body::from_stream(stream).into_response(),
        Some(Reason::BodyTimeout),
    )
    .await;
    // A declared body longer than the stream keeps the failure after headers.
    let response = (
        [(header::CONTENT_LENGTH, "100")],
        Body::from_stream(futures_util::stream::once(async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok::<_, std::io::Error>("{")
        })),
    )
        .into_response();
    fetch_response(response, Some(Reason::ResponseBody)).await;
}

#[tokio::test]
async fn catalogue_connection_and_configuration_failures_are_attributed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    for (origin, dependency, reason) in [
        (
            origin.as_str(),
            Dependency::ModelGateway,
            Reason::Connection,
        ),
        ("data:fixture", Dependency::Local, Reason::Configuration),
    ] {
        let catalog =
            ChronoAgentModels::new(origin, Arc::new(super::tests::NoCredentials)).unwrap();
        let (result, failure) = capture(catalog.fetch("chrono-llm-public", "fixture")).await;
        assert!(result.is_err());
        assert_eq!(failure, Some(Failure::new(dependency, reason)));
    }
}

#[tokio::test]
async fn catalogue_header_timeout_and_transport_failure_are_attributed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let mut catalog =
        ChronoAgentModels::new(&origin, Arc::new(super::tests::NoCredentials)).unwrap();
    Arc::get_mut(&mut catalog).unwrap().http = reqwest::Client::builder()
        .timeout(Duration::from_millis(100))
        .build()
        .unwrap();
    let (result, failure) = capture(catalog.fetch("chrono-llm-public", "fixture")).await;
    assert!(result.is_err());
    assert_eq!(
        failure,
        Some(Failure::new(Dependency::ModelGateway, Reason::Timeout))
    );
    server.abort();
    let catalog = ChronoAgentModels::new(
        "ftp://fixture.invalid",
        Arc::new(super::tests::NoCredentials),
    )
    .unwrap();
    let (result, failure) = capture(catalog.fetch("chrono-llm-public", "fixture")).await;
    assert!(result.is_err());
    assert_eq!(
        failure,
        Some(Failure::new(Dependency::ModelGateway, Reason::Transport))
    );
}
