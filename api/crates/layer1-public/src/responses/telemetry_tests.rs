use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, Kind};
#[tokio::test]
async fn responses_initial_output_and_read_failure_have_exact_counts() {
    isolated_test(async {
        let (state, agent) = completed().await;
        let caller = layer2_core::Caller::new(
            layer2_core::SubjectId("owner-a".into()),
            layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        );
        let stream = super::super::stream::open(state, caller.clone(), agent, 3, None);
        assert_eq!(snapshot(Kind::Responses), (1, [0; 3]));
        let bytes = to_bytes(stream.into_body(), 1024 * 1024).await.unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(text.contains("response.completed"));
        assert_eq!(snapshot(Kind::Responses), (0, [0, 1, 0]));
        let (state, _) = conversation_fixture().await;
        let agent = seeded_agent("agt_unreadable", AgentStatus::Working, 10, Some(3));
        let stream = super::super::stream::open(state, caller, agent, 3, None);
        assert_eq!(snapshot(Kind::Responses).0, 1);
        let bytes = to_bytes(stream.into_body(), 65536).await.unwrap();
        assert!(bytes.is_empty());
        assert_eq!(snapshot(Kind::Responses), (0, [0, 1, 1]));
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn responses_refresh_counts_quiet_output_and_failure_in_both_read_paths() {
    use futures_util::StreamExt;
    use layer2_core::sandbox::store::AgentStore;
    use std::time::Duration;

    for authorized in [false, true] {
        isolated_test(async {
            let (state, store) = crate::rest::agents_tests::authorized_conversation_fixture().await;
            let mut agent = seeded_agent("agt_telemetry", AgentStatus::Working, 4, Some(3));
            insert_agent(
                &store,
                agent.clone(),
                vec![
                    AgentLogKind::TurnStarted {
                        model: None,
                        turn_sequence: 3,
                        origin_key: None,
                    },
                    AgentLogKind::OwnerMessage {
                        content: "test input".into(),
                    },
                ],
            )
            .await;
            let context = crate::agent_v2::fixtures::context("owner-a");
            let response = if authorized {
                super::super::stream::open_authorized(
                    state,
                    crate::AuthenticatedResourceCaller {
                        context,
                        credential: None,
                    },
                    agent.clone(),
                    3,
                    None,
                )
                .await
            } else {
                super::super::stream::open(state, context.caller, agent.clone(), 3, None)
            };
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(snapshot(Kind::Responses), (1, [0; 3]));
            let mut body = response.into_body().into_data_stream();
            for event in ["response.created", "response.in_progress"] {
                let frame = body.next().await.unwrap().unwrap();
                assert!(std::str::from_utf8(&frame).unwrap().contains(event));
                assert_eq!(snapshot(Kind::Responses), (1, [0, 1, 0]));
            }
            // Unlike the other streams, v1 reads immediately before sleeping:
            // quiet polls occur at 0, 100, 200 and 300 ms.
            assert!(
                tokio::time::timeout(Duration::from_millis(350), body.next())
                    .await
                    .is_err()
            );
            assert_eq!(snapshot(Kind::Responses), (1, [4, 1, 0]));
            let revision = agent.revision;
            agent.revision += 1;
            agent.conversation.last_seq += 1; // absent row: refresh must fail closed
            store.save_agent(revision, agent).await.unwrap();
            assert!(body.next().await.is_none());
            assert_eq!(snapshot(Kind::Responses), (0, [4, 1, 1]));
        })
        .await;
    }
}

#[tokio::test(start_paused = true)]
async fn responses_without_a_projectable_turn_count_quiet_polls_and_drop() {
    use futures_util::StreamExt;
    use std::time::Duration;

    isolated_test(async {
        let (state, store) = conversation_fixture().await;
        let agent = seeded_agent("agt_telemetry", AgentStatus::Working, 4, Some(3));
        insert_agent(
            &store,
            agent.clone(),
            vec![
                AgentLogKind::TurnStarted {
                    model: None,
                    turn_sequence: 3,
                    origin_key: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "test input".into(),
                },
            ],
        )
        .await;
        let caller = crate::agent_v2::fixtures::context("owner-a").caller;
        let response = super::super::stream::open(state, caller, agent, 5, None);
        assert_eq!(snapshot(Kind::Responses), (1, [0; 3]));
        let mut body = response.into_body().into_data_stream();
        assert!(
            tokio::time::timeout(Duration::from_millis(350), body.next())
                .await
                .is_err()
        );
        assert_eq!(snapshot(Kind::Responses), (1, [4, 0, 0]));
        drop(body);
        assert_eq!(snapshot(Kind::Responses), (0, [4, 0, 0]));
    })
    .await;
}
