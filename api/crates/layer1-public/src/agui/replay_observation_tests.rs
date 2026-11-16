use super::*;
use crate::{
    observation_test_support::{attach, custody},
    rest::agents_tests::{authorized_conversation_fixture, insert_agent, seeded_agent},
};
use futures_util::StreamExt;
use layer2_core::sandbox::agent_observation::observation_scope;
use std::sync::atomic::Ordering;

#[tokio::test]
async fn shared_replay_matches_wire_and_resumes_between_text_frames() {
    let (state, store) = authorized_conversation_fixture().await;
    let mut state = state.with_bounded_replay(true);
    let agent = seeded_agent("agt_observe", AgentStatus::Ready, 6, None);
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
                content: "input".into(),
            },
            AgentLogKind::AgentMessage {
                content: "😀".repeat(100_000),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
        ],
    )
    .await;
    let caller = Caller::new(
        agent.owner.clone(),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    );
    let legacy = open(
        state.clone(),
        caller.clone(),
        agent.id.clone(),
        0,
        false,
        None,
        None,
    )
    .await;
    let expected = axum::body::to_bytes(legacy.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    (state.reads, _) = attach(&state.reads, store);
    let response = observation_scope(
        custody(&agent),
        open(
            state.clone(),
            caller.clone(),
            agent.id.clone(),
            0,
            false,
            None,
            None,
        ),
    )
    .await;
    let mut body = response.into_body().into_data_stream();
    let mut wire = Vec::new();
    let mut acknowledged = 0;
    while let Some(frame) = body.next().await {
        let frame = frame.unwrap();
        let text = std::str::from_utf8(&frame).unwrap();
        if let Some(id) = text.lines().find_map(|line| line.strip_prefix("id: ")) {
            acknowledged = id.parse().unwrap();
        }
        wire.extend_from_slice(&frame);
        if text.contains("TEXT_MESSAGE_CONTENT") && text.contains("msg_5") {
            break;
        }
    }
    assert_eq!(acknowledged, 4);
    drop(body);
    let resumed = observation_scope(
        custody(&agent),
        open(
            state.clone(),
            caller.clone(),
            agent.id.clone(),
            acknowledged,
            false,
            None,
            None,
        ),
    )
    .await;
    let resumed = axum::body::to_bytes(resumed.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    let resumed = std::str::from_utf8(&resumed).unwrap();
    assert!(resumed.contains("TEXT_MESSAGE_START") && resumed.contains("TEXT_MESSAGE_END"));
    let complete = observation_scope(
        custody(&agent),
        open(state, caller, agent.id.clone(), 0, false, None, None),
    )
    .await;
    assert_eq!(
        axum::body::to_bytes(complete.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap(),
        expected
    );
    assert!(expected.starts_with(&wire));
}

#[tokio::test]
async fn shared_owner_replay_fences_buffered_frames_after_deletion_or_read_timeout() {
    for deleted in [false, true] {
        let (state, store) = authorized_conversation_fixture().await;
        let mut state = state.with_bounded_replay(true);
        let agent = seeded_agent("agt_observe_failure", AgentStatus::Ready, 6, None);
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
                    content: "input".into(),
                },
                AgentLogKind::AgentMessage {
                    content: "retained output".into(),
                },
                AgentLogKind::TurnCompleted { turn_sequence: 3 },
            ],
        )
        .await;
        let (reads, facts) = attach(&state.reads, store.clone());
        state.reads = reads;
        let caller = Caller::new(
            agent.owner.clone(),
            layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
        );
        let response = observation_scope(
            custody(&agent),
            open(state, caller, agent.id.clone(), 0, false, None, None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body().into_data_stream();
        let first = body.next().await.unwrap().unwrap();
        assert!(!std::str::from_utf8(&first).unwrap().contains("RUN_ERROR"));
        if deleted {
            // Mongo coverage removes the retained row; a terminal Deleted
            // status alone intentionally preserves owner-visible history.
            facts.missing.store(true, Ordering::SeqCst);
        } else {
            facts.unavailable.store(true, Ordering::SeqCst);
        }
        // Resume immediately: neither owner path nor a buffered page may use
        // the earlier observation as authority for its next frame.
        assert!(body.next().await.is_none_or(|frame| frame.is_err()));
    }
}
