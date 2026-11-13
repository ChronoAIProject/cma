use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, Kind};
use futures_util::StreamExt;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
#[tokio::test(start_paused = true)]
async fn agui_refresh_counts_quiet_output_and_failure_in_both_read_paths() {
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
            let caller = Caller::new(
                agent.owner.clone(),
                layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
            );
            let response = if authorized {
                super::super::authorized::open(
                    state.clone(),
                    crate::AuthenticatedResourceCaller {
                        context: crate::agent_v2::fixtures::context("owner-a"),
                        credential: None,
                    },
                    agent.id.clone(),
                    4,
                    true,
                    None,
                    None,
                )
                .await
            } else {
                super::super::open(
                    state.clone(),
                    caller.clone(),
                    agent.id.clone(),
                    4,
                    true,
                    None,
                    None,
                )
                .await
            };
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(snapshot(Kind::Agui), (1, [0; 3]));
            let mut body = response.into_body().into_data_stream();
            assert!(
                tokio::time::timeout(Duration::from_millis(350), body.next())
                    .await
                    .is_err()
            );
            assert_eq!(snapshot(Kind::Agui), (1, [3, 0, 0]));
            store
                .append_agent_log(
                    &agent.id,
                    "2026-11-08T00:00:00Z".into(),
                    AgentLogKind::AgentMessage {
                        content: "test-visible-output".into(),
                    },
                )
                .await
                .unwrap();
            let revision = agent.revision;
            agent.revision += 1;
            agent.conversation.last_seq += 1;
            store.save_agent(revision, agent.clone()).await.unwrap();
            assert!(state.reads.log_snapshot(&caller, &agent.id).await.is_ok());
            for _ in 0..3 {
                assert!(body.next().await.unwrap().is_ok());
            }
            assert_eq!(snapshot(Kind::Agui), (1, [3, 1, 0]));
            let revision = agent.revision;
            agent.revision += 1;
            agent.conversation.last_seq += 1;
            store.save_agent(revision, agent).await.unwrap(); // now head claims an absent row
            assert!(body.next().await.is_none());
            assert_eq!(snapshot(Kind::Agui), (0, [3, 1, 1]));
        })
        .await;
    }
}
