use super::*;

#[tokio::test]
async fn semantic_close_counts_actual_body_disposition_once() {
    use axum::response::IntoResponse;
    use futures_util::StreamExt;
    use layer1_public::stream_telemetry::ContractKind;
    use CloseReason::*;
    #[derive(Clone, Copy)]
    enum End {
        Drop,
        FrameThenDrop,
        Eof,
        Error,
        Empty,
    }
    let cases = [
        (Some(NormalTerminal), End::Drop, "drop_unknown"),
        (None, End::FrameThenDrop, "drop_unknown"),
        (Some(NormalTerminal), End::FrameThenDrop, "drop_unknown"),
        (Some(NormalTerminal), End::Eof, "normal_terminal"),
        (Some(FiniteReplay), End::Eof, "finite_replay"),
        (
            Some(AuthorizationRevoked),
            End::Eof,
            "authorization_revoked",
        ),
        (
            Some(AuthorizationUnavailable),
            End::Eof,
            "authorization_unavailable",
        ),
        (Some(DependencyError), End::Eof, "dependency_error"),
        (
            Some(DependencyError),
            End::FrameThenDrop,
            "dependency_error",
        ),
        (Some(PaginationRequired), End::Eof, "pagination_required"),
        (None, End::Eof, "eof_unexplained"),
        (None, End::Empty, "eof_unexplained"),
        (Some(NormalTerminal), End::Error, "server_error"),
        (Some(DependencyError), End::Error, "dependency_error"),
    ];
    for (reason, end, expected) in cases {
        let recorder = recorder();
        let handle = recorder.handle();
        let _recording = metrics::set_default_local_recorder(&recorder);
        let contract = Contract::new(ContractKind::Responses);
        let retained = contract.clone();
        let app = Router::new()
            .route(
                "/api/v1/responses",
                get(move || {
                    let contract = contract.clone();
                    async move {
                        let evidence = contract.clone();
                        let frames = stream::iter([0]).filter_map(move |_| {
                            let evidence = evidence.clone();
                            async move {
                                if let Some(reason) = reason {
                                    evidence.record(reason);
                                }
                                match end {
                                    End::Empty => None,
                                    End::Error => {
                                        Some(Err(std::io::Error::other("private detail")))
                                    }
                                    _ => {
                                        Some(Ok(Bytes::from_static(b"event: error\ndata: {}\n\n")))
                                    }
                                }
                            }
                        });
                        let mut response = (
                            [(header::CONTENT_TYPE, "text/event-stream")],
                            Body::from_stream(frames),
                        )
                            .into_response();
                        response.extensions_mut().insert(contract);
                        response
                    }
                }),
            )
            .layer(axum::middleware::from_fn(observe));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/responses?private-query")
                    .header("x-consumer", "forged-private")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        drop(retained.clone());
        assert!(!handle.render().contains("cma_sse_contract_closed_total"));
        let mut body = response.into_body();
        match end {
            End::Drop => {}
            End::FrameThenDrop => {
                assert!(
                    std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                        .await
                        .unwrap()
                        .is_ok()
                );
                assert!(!handle.render().contains("cma_sse_contract_closed_total"));
            }
            End::Error => {
                assert!(
                    std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                        .await
                        .unwrap()
                        .is_err()
                );
            }
            End::Eof | End::Empty => {
                while let Some(frame) =
                    std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await
                {
                    frame.unwrap();
                }
                assert!(
                    std::future::poll_fn(|cx| Pin::new(&mut body).poll_frame(cx))
                        .await
                        .is_none()
                );
            }
        }
        drop(body);
        drop(retained);
        let text = handle.render();
        let opens: Vec<_> = text
            .lines()
            .filter(|line| line.starts_with("cma_sse_contract_opened_total{"))
            .collect();
        assert_eq!(opens, ["cma_sse_contract_opened_total{stream_kind=\"responses\",consumer=\"unknown\",protocol=\"responses\"} 1"]);
        let closes: Vec<_> = text
            .lines()
            .filter(|line| line.starts_with("cma_sse_contract_closed_total{"))
            .collect();
        assert_eq!(closes, [format!("cma_sse_contract_closed_total{{stream_kind=\"responses\",consumer=\"unknown\",protocol=\"responses\",reason=\"{expected}\"}} 1")]);
        assert_eq!(
            text.lines()
                .filter(|line| line.starts_with("cma_sse_observation_closed_total{"))
                .count(),
            1
        );
        for private in ["private-query", "forged-private", "private detail"] {
            assert!(!text.contains(private));
        }
    }
}

#[tokio::test]
async fn semantic_denominator_excludes_unsuccessful_and_non_sse_responses() {
    use axum::response::IntoResponse;
    for (status, content_type) in [
        (StatusCode::BAD_REQUEST, "text/event-stream"),
        (StatusCode::OK, "application/json"),
    ] {
        let recorder = recorder();
        let handle = recorder.handle();
        let _recording = metrics::set_default_local_recorder(&recorder);
        let app = Router::new()
            .route(
                "/sse",
                get(move || async move {
                    (status, [(header::CONTENT_TYPE, content_type)], "{}").into_response()
                }),
            )
            .layer(axum::middleware::from_fn(observe));
        let response = app
            .oneshot(Request::builder().uri("/sse").body(Body::empty()).unwrap())
            .await
            .unwrap();
        axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert!(!handle.render().contains("cma_sse_contract_"));
    }
}

#[tokio::test]
async fn coverage_exposes_supported_and_unclassified_streams() {
    let recorder = recorder();
    let handle = recorder.handle();
    let _recording = metrics::set_default_local_recorder(&recorder);
    layer1_public::stream_telemetry::emit();
    let text = handle.render();
    for kind in ["agui", "responses", "responses_v2"] {
        assert!(text.contains(&format!(
            "cma_sse_contract_coverage{{stream_kind=\"{kind}\"}} 1"
        )));
    }
    for kind in ["agent_events_v2", "launch_events"] {
        assert!(text.contains(&format!(
            "cma_sse_contract_coverage{{stream_kind=\"{kind}\"}} 0"
        )));
    }
}
