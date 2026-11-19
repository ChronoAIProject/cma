use super::*;
use layer2_core::sandbox::TypedFailure;

#[tokio::test]
async fn retained_provider_failure_is_safe_after_index_certification_and_on_replay() {
    for class in ["agent_turn_failed", "PRIVATE_CLASS_SENTINEL"] {
        let mut kinds = beginning();
        kinds.extend([
            tool("call", "completed"),
            output("call", "<p>503 tool output remains</p>"),
            message("<p>524 assistant output remains</p>"),
            AgentLogKind::TurnFailed { turn_sequence: 3, failure: TypedFailure {
                class: class.into(), detail: "unexpected status 503: <html>PRIVATE_BODY_SENTINEL https://failure.invalid X-Private-Header</html>".into(),
            } },
        ]);
        let f = make(kinds).await;
        let before_rows = serde_json::to_vec(&f.source.data.rows).unwrap();
        let before_facts =
            serde_json::to_vec(&f.source.data.metadata.turn.as_ref().unwrap().facts).unwrap();
        let reading = f.reading.clone();
        let actual = f
            .observe(move |source| {
                Box::pin(async move {
                    projection::indexed::complete(
                        source,
                        &crate::agent_page::AgentPageOrigin::test(),
                        reading,
                    )
                    .await
                })
            })
            .await
            .unwrap()
            .unwrap();
        safe(&serde_json::to_value(&actual).unwrap());
        assert_eq!(
            serde_json::to_value(&actual).unwrap(),
            serde_json::to_value(f.golden()).unwrap()
        );
        let events = projection::events(&f.golden(), true);
        for next in 0..=events.len() {
            let suffix = super::parity::suffix(&f, next as u64).await;
            safe(&serde_json::to_value(&suffix).unwrap());
            assert_eq!(suffix, events[next..]);
        }
        let agent = f.agent.clone();
        let legacy = f
            .observe(move |source| {
                Box::pin(async move {
                    crate::responses::retrieve::indexed::complete(
                        source,
                        &crate::agent_page::AgentPageOrigin::test(),
                        &agent,
                        3,
                    )
                    .await
                })
            })
            .await
            .unwrap()
            .unwrap();
        safe(&serde_json::to_value(&legacy).unwrap());
        let (legacy_events, _) = super::legacy_bounds::collect(&f, true).await;
        safe(&serde_json::to_value(&legacy_events).unwrap());
        assert_eq!(legacy_events.last().unwrap()["type"], "response.failed");
        let router = crate::responses::router(f.state.clone());
        let (status, body) = crate::rest::agents_tests::json_response(
            &router,
            request(
                Method::GET,
                "/api/v1/responses/resp_agt_indexed_3",
                "owner-a",
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        safe(&body);

        let wire = serde_json::to_string(&actual).unwrap();
        assert!(
            wire.contains("503 tool output remains")
                && wire.contains("524 assistant output remains")
        );
        assert_eq!(
            serde_json::to_vec(&f.source.data.rows).unwrap(),
            before_rows
        );
        assert_eq!(
            serde_json::to_vec(&f.source.data.metadata.turn.as_ref().unwrap().facts).unwrap(),
            before_facts
        );
        assert!(String::from_utf8(before_rows)
            .unwrap()
            .contains("PRIVATE_BODY_SENTINEL"));
    }
}

fn safe(value: &Value) {
    let wire = value.to_string();
    for private in [
        "PRIVATE_BODY_SENTINEL",
        "PRIVATE_CLASS_SENTINEL",
        "failure.invalid",
        "X-Private-Header",
    ] {
        assert!(
            !wire.contains(private),
            "public payload exposed {private}: {wire}"
        );
    }
}
