use super::*;
use layer2_core::sandbox::{store::AgentLogStore, TypedFailure};

#[tokio::test]
async fn retained_provider_failure_v2_http_get_and_events_are_safe_for_both_readers() {
    for indexed in [false, true] {
        let mut kinds = beginning();
        kinds.extend([message("<p>503 assistant output remains</p>"), AgentLogKind::TurnFailed {
            turn_sequence: 3, failure: TypedFailure { class: "PRIVATE_CLASS_SENTINEL".into(), detail: "unexpected status 503: <html>PRIVATE_BODY_SENTINEL https://failure.invalid X-Private-Header</html>".into() },
        }]);
        let f = make(kinds).await;
        let mut reads = f.reads.clone();
        if !indexed {
            for row in f.source.data.rows.values() {
                f.store
                    .append_agent_log(&f.agent.id, row.appended_at.clone(), row.kind.clone())
                    .await
                    .unwrap();
            }
            reads = reads.with_response_logs(None);
        }
        let commands = AgentCommandService::new(
            Arc::new(BindingStore {
                inner: f.store.clone(),
                bind_after_response_reads: 0,
                bound: AtomicBool::new(true),
                response_reads: AtomicUsize::new(0),
            }),
            f.access.clone(),
            Arc::new(Authority),
        );
        let router = crate::responses::v2::router(crate::responses::v2::ResponsesV2State::new(
            f.state.agent_page_origin.clone(),
            commands,
            reads,
        ));
        let response = router
            .clone()
            .oneshot(request(
                Method::POST,
                "/api/v2/responses",
                "owner-a",
                Some("safe-failure"),
                Some(json!({"conversation":"agt_indexed","input":"Begin","stream":true})),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let id = response.headers()["x-cma-response-id"]
            .to_str()
            .unwrap()
            .to_owned();
        drop(response);
        for suffix in ["", "/events"] {
            let (status, body) = crate::rest::agents_tests::response(
                &router,
                request(
                    Method::GET,
                    &format!("/api/v2/responses/{id}{suffix}"),
                    "owner-a",
                    None,
                    None,
                ),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let wire = std::str::from_utf8(&body).unwrap();
            for private in ["SENTINEL", "failure.invalid", "X-Private-Header"] {
                assert!(
                    !wire.contains(private),
                    "public payload exposed {private}: {wire}"
                );
            }
            assert!(wire.contains("agent_turn_failed") && wire.contains("The Agent run failed."));
            assert!(wire.contains("503 assistant output remains"));
        }
    }
}
