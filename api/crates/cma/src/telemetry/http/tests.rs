use super::*;
use axum::{
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use futures_util::stream;
use tower::ServiceExt;

fn recorder() -> metrics_exporter_prometheus::PrometheusRecorder {
    metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder()
}

#[tokio::test]
async fn attempts_use_route_templates_and_count_bytes_without_claiming_terminals() {
    let recorder = recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let app = Router::new()
        .route("/agents/{id}", get(|| async { "hello" }))
        .layer(axum::middleware::from_fn(observe));
    for id in ["private-agent-one", "private-agent-two"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/agents/{id}?token=private-query"))
                    .header("x-request-id", "private-correlation")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap(),
            "hello"
        );
    }
    let text = handle.render();
    assert!(text.contains("cma_http_attempts_total{route=\"/agents/{id}\",method=\"GET\"} 2"));
    assert!(text
        .contains("cma_http_response_body_bytes_total{route=\"/agents/{id}\",method=\"GET\"} 10"));
    assert!(text.contains("reason=\"eof\"} 2"));
    for private in [
        "private-agent",
        "private-query",
        "private-correlation",
        "terminal",
    ] {
        assert!(
            !text.contains(private),
            "metrics leaked or misclassified: {private}"
        );
    }
}

#[tokio::test]
async fn sse_body_drop_and_body_error_are_observation_outcomes_after_http_200() {
    let recorder = recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let app = Router::new()
        .route(
            "/sse",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    Body::from_stream(stream::pending::<Result<Bytes, std::io::Error>>()),
                )
            }),
        )
        .route(
            "/error",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    Body::from_stream(stream::once(async {
                        Err::<Bytes, _>(std::io::Error::other("private upstream detail"))
                    })),
                )
            }),
        )
        .layer(axum::middleware::from_fn(observe));
    let response = app
        .clone()
        .oneshot(Request::builder().uri("/sse").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/error")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(axum::body::to_bytes(response.into_body(), 1024)
        .await
        .is_err());
    let text = handle.render();
    assert!(text.contains("cma_sse_observation_closed_total{route=\"/sse\",reason=\"dropped\"} 1"));
    assert!(text.contains("cma_sse_observation_closed_total{route=\"/error\",reason=\"error\"} 1"));
    assert!(!text.contains("private upstream"));
    assert!(!text.contains("terminal"));
}

#[tokio::test]
async fn frame_wrapper_preserves_trailers() {
    let recorder = recorder();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let mut trailers = axum::http::HeaderMap::new();
    trailers.insert("x-trailer", "kept".parse().unwrap());
    let frames = stream::iter([
        Ok::<_, std::io::Error>(Frame::data(Bytes::from_static(b"one"))),
        Ok(Frame::trailers(trailers.clone())),
    ]);
    let mut observed = ObservedBody {
        body: Body::new(http_body_util::StreamBody::new(frames)),
        bytes: metrics::counter!("cma_http_response_body_bytes_total", "route" => "/frames", "method" => "GET"),
        route: "/frames".into(),
        method: "GET",
        sse: false,
        started: Instant::now(),
        finished: false,
    };
    let frame = std::future::poll_fn(|cx| Pin::new(&mut observed).poll_frame(cx))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_data().unwrap(), Bytes::from_static(b"one"));
    let frame = std::future::poll_fn(|cx| Pin::new(&mut observed).poll_frame(cx))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.into_trailers().unwrap(), trailers);
    assert!(
        std::future::poll_fn(|cx| Pin::new(&mut observed).poll_frame(cx))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn dropping_request_before_headers_keeps_attempt_and_cancellation_counts() {
    let recorder = recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    {
        let app = Router::new()
            .route("/cancel", get(std::future::pending::<()>))
            .layer(axum::middleware::from_fn(observe));
        let mut request = Box::pin(
            app.oneshot(
                Request::builder()
                    .uri("/cancel")
                    .body(Body::empty())
                    .unwrap(),
            ),
        );
        assert!(futures_util::poll!(&mut request).is_pending());
    }
    let text = handle.render();
    assert!(text
        .contains("cma_http_before_headers_cancelled_total{route=\"/cancel\",method=\"GET\"} 1"));
    assert!(text.contains("cma_http_in_flight{route=\"/cancel\",method=\"GET\"} 0"));
}

#[tokio::test(start_paused = true)]
async fn long_quiet_sse_is_not_closed_or_classified_as_a_terminal_by_elapsed_time() {
    let recorder = recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    let app = Router::new()
        .route(
            "/long",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/event-stream")],
                    Body::from_stream(stream::pending::<Result<Bytes, std::io::Error>>()),
                )
            }),
        )
        .layer(axum::middleware::from_fn(observe));
    let response = app
        .oneshot(Request::builder().uri("/long").body(Body::empty()).unwrap())
        .await
        .unwrap();
    tokio::time::advance(std::time::Duration::from_secs(601)).await;
    assert!(!handle.render().contains("cma_sse_observation_closed_total"));
    drop(response);
    assert!(handle
        .render()
        .contains("cma_sse_observation_closed_total{route=\"/long\",reason=\"dropped\"} 1"));
}
