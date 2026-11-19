use super::*;
use crate::rest::agents_tests::{authorized_conversation_fixture, insert_agent, seeded_agent};

#[tokio::test]
async fn closed_flag_pins_the_original_wire_in_both_owner_paths() {
    for authorized in [false, true] {
        let (state, store) = authorized_conversation_fixture().await;
        assert!(!state.bounded_replay);
        let agent = seeded_agent("agt_wire", AgentStatus::Ready, 6, None);
        let content = "λ".repeat(8193);
        insert_agent(
            &store,
            agent.clone(),
            vec![
                AgentLogKind::TurnStarted {
                    turn_sequence: 3,
                    origin_key: None,
                    model: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "input".into(),
                },
                AgentLogKind::AgentMessage {
                    content: content.clone(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let response = if authorized {
            authorized::open(
                state,
                crate::AuthenticatedResourceCaller {
                    context: crate::agent_v2::fixtures::context("owner-a"),
                    credential: None,
                },
                agent.id,
                4,
                false,
                None,
                None,
            )
            .await
        } else {
            open(
                state,
                Caller::new(
                    agent.owner,
                    layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
                ),
                agent.id,
                4,
                false,
                None,
                None,
            )
            .await
        };
        assert_eq!(response.status(), StatusCode::OK);
        let wire = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        let at = agent.updated_at;
        let expected = format!(concat!(
            "data: {{\"type\":\"TEXT_MESSAGE_START\",\"threadId\":\"agt_wire\",\"runId\":\"3\",\"messageId\":\"msg_5\",\"role\":\"assistant\",\"timestamp\":\"{at}\"}}\n\n",
            "data: {{\"type\":\"TEXT_MESSAGE_CONTENT\",\"threadId\":\"agt_wire\",\"runId\":\"3\",\"messageId\":\"msg_5\",\"delta\":\"{content}\",\"timestamp\":\"{at}\"}}\n\n",
            "data: {{\"type\":\"TEXT_MESSAGE_END\",\"threadId\":\"agt_wire\",\"runId\":\"3\",\"messageId\":\"msg_5\",\"timestamp\":\"{at}\"}}\nid: 5\n\n",
            "data: {{\"type\":\"RUN_FINISHED\",\"threadId\":\"agt_wire\",\"runId\":\"3\",\"outcome\":{{\"type\":\"complete\"}},\"timestamp\":\"{at}\"}}\nid: 6\n\n"
        ), at = at, content = content);
        assert_eq!(String::from_utf8(wire.to_vec()).unwrap(), expected);
    }
}

#[tokio::test]
async fn selected_run_seeks_across_row_and_byte_caps_in_both_owner_paths() {
    for authorized in [false, true] {
        for large in [false, true] {
            let (state, store) = authorized_conversation_fixture().await;
            let state = state.with_bounded_replay(true);
            let run = if large { 8 } else { 405 };
            let agent = seeded_agent("agt_seek", AgentStatus::Ready, run + 3, None);
            let mut kinds = vec![
                AgentLogKind::TurnStarted {
                    turn_sequence: 3,
                    origin_key: None,
                    model: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "old input".into(),
                },
            ];
            kinds.resize(
                (run - 4) as usize,
                AgentLogKind::AgentMessage {
                    content: if large {
                        "λ".repeat(1_100_000)
                    } else {
                        "old output".into()
                    },
                },
            );
            kinds.push(AgentLogKind::TurnCompleted { turn_sequence: 3 });
            kinds.extend([
                AgentLogKind::TurnStarted {
                    turn_sequence: run,
                    origin_key: None,
                    model: None,
                },
                AgentLogKind::OwnerMessage {
                    content: "selected input".into(),
                },
                AgentLogKind::AgentMessage {
                    content: "selected output".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: run },
            ]);
            insert_agent(&store, agent.clone(), kinds).await;
            for selected in [run, run - 1, run + 1, run + 4, 0] {
                let response = if authorized {
                    authorized::open(
                        state.clone(),
                        crate::AuthenticatedResourceCaller {
                            context: crate::agent_v2::fixtures::context("owner-a"),
                            credential: None,
                        },
                        agent.id.clone(),
                        0,
                        false,
                        Some(selected),
                        None,
                    )
                    .await
                } else {
                    open(
                        state.clone(),
                        Caller::new(
                            agent.owner.clone(),
                            layer2_core::Authorization::new([
                                layer2_core::Capability::UseOwnSandboxes,
                            ]),
                        ),
                        agent.id.clone(),
                        0,
                        false,
                        Some(selected),
                        None,
                    )
                    .await
                };
                let status = response.status();
                let bytes = axum::body::to_bytes(response.into_body(), 65536)
                    .await
                    .unwrap();
                if selected != run {
                    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
                    let problem: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                    assert_eq!(problem["code"], "instance_service_unavailable");
                    continue;
                }
                assert_eq!(status, StatusCode::OK);
                let wire = String::from_utf8(bytes.to_vec()).unwrap();
                assert!(!wire.contains("old output") && !wire.contains("old input"));
                assert!(wire.contains("selected output") && wire.contains("RUN_FINISHED"));
                let ids: Vec<u64> = wire
                    .lines()
                    .filter_map(|line| line.strip_prefix("id: "))
                    .map(|id| id.parse().unwrap())
                    .collect();
                assert_eq!(ids, (run..=run + 3).collect::<Vec<_>>());
            }
        }
    }
}
