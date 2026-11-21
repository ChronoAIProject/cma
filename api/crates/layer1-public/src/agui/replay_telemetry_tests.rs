use super::*;
use crate::stream_telemetry::{isolated_test, snapshot, BodyEnd, CloseReason, Contract, Kind};
use futures_util::StreamExt;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
#[tokio::test(start_paused = true)]
async fn agui_refresh_counts_quiet_output_and_failure_in_both_read_paths() {
    for (authorized, bounded) in [(false, false), (true, false), (false, true), (true, true)] {
        isolated_test(async {
            let (state, store) = crate::rest::agents_tests::authorized_conversation_fixture().await;
            let state = state.with_bounded_replay(bounded);
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
            let evidence = response.extensions().get::<Contract>().unwrap().clone();
            let mut body = response.into_body().into_data_stream();
            assert!(tokio::time::timeout(
                Duration::from_millis(if bounded { 3500 } else { 350 }),
                body.next()
            )
            .await
            .is_err());
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
            if bounded {
                assert!(body.next().await.unwrap().is_err());
            }
            assert!(body.next().await.is_none());
            assert_eq!(
                evidence.close_reason(BodyEnd::Eof),
                CloseReason::ServerError
            );
            assert_eq!(snapshot(Kind::Agui), (0, [3, 1, 1]));
        })
        .await;
    }
}

#[tokio::test]
async fn finite_replay_and_selected_run_completion_do_not_classify_follow_drops() {
    for bounded in [false, true] {
        for (follow, run, expected) in [
            (false, None, CloseReason::FiniteReplay),
            (true, Some(3), CloseReason::NormalTerminal),
            (true, None, CloseReason::DropUnknown),
        ] {
            let (state, store) = crate::rest::agents_tests::conversation_fixture().await;
            let state = state.with_bounded_replay(bounded);
            let agent = seeded_agent("agt_semantics", AgentStatus::Ready, 5, None);
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
                        content: "synthetic input".into(),
                    },
                    AgentLogKind::TurnCompleted { turn_sequence: 3 },
                ],
            )
            .await;
            let caller = crate::agent_v2::fixtures::context("owner-a").caller;
            let response = super::super::open(state, caller, agent.id, 0, follow, run, None).await;
            assert_eq!(response.status(), StatusCode::OK);
            let evidence = response.extensions().get::<Contract>().unwrap().clone();
            let mut body = response.into_body().into_data_stream();
            let mut wire = String::new();
            while let Some(frame) = body.next().await {
                wire.push_str(std::str::from_utf8(&frame.unwrap()).unwrap());
                if follow && run.is_none() && wire.contains("RUN_FINISHED") {
                    break;
                }
            }
            assert!(wire.contains("RUN_FINISHED"));
            drop(body);
            assert_eq!(
                evidence.close_reason(if run.is_none() && follow {
                    BodyEnd::Dropped
                } else {
                    BodyEnd::Eof
                }),
                expected
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn buffered_terminal_cannot_hide_lost_authority() {
    use futures_util::StreamExt;
    use layer2_core::sandbox::store::{SandboxStore, SandboxWorkerStore};
    for bounded in [false, true] {
        let (state, store) = crate::rest::agents_tests::authorized_conversation_fixture().await;
        let agent = seeded_agent("agt_terminal_fence", AgentStatus::Ready, 5, None);
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
                    content: "synthetic input".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let resource = crate::AuthenticatedResourceCaller {
            context: crate::agent_v2::fixtures::context("owner-a"),
            credential: None,
        };
        let response = super::super::authorized::open(
            state.with_bounded_replay(bounded),
            resource,
            agent.id.clone(),
            2,
            true,
            Some(3),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let evidence = response.extensions().get::<Contract>().unwrap().clone();
        let mut body = response.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert!(!std::str::from_utf8(&first)
            .unwrap()
            .contains("RUN_FINISHED"));

        let mut sandbox = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
        let revision = sandbox.revision;
        sandbox.status = layer2_core::sandbox::SandboxStatus::Retired;
        sandbox.retired_at = Some("2026-09-05T00:00:00Z".into());
        sandbox.purge_after = sandbox.retired_at.clone();
        sandbox.revision += 1;
        store.save_sandbox(revision, sandbox).await.unwrap();
        store
            .purge_sandbox(&agent.sandbox_id, revision + 1, "2026-09-05T00:00:00Z")
            .await
            .unwrap();
        // Personal-owner bounded replay refreshes its header once per second.
        tokio::time::advance(Duration::from_secs(1)).await;
        assert!(body.next().await.is_none());
        assert_eq!(
            evidence.close_reason(BodyEnd::Eof),
            CloseReason::AuthorizationUnavailable
        );
    }
}
