use layer2_core::sandbox::AgentLiveFragment;
#[path = "replay_telemetry_tests.rs"]
mod telemetry_tests;

use super::*;
use crate::rest::agents_tests::{conversation_fixture, insert_agent, seeded_agent};
use layer2_core::sandbox::{AgentStatus, AgentUpdateRecorder, DurableAgentUpdateRecorder};

async fn following() -> (Following, Agent) {
    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_live", AgentStatus::Working, 5, Some(3));
    let recorder = DurableAgentUpdateRecorder::new(store);
    let watching = recorder.subscribe_live(&agent.id);
    let caller = Caller::new(
        agent.owner.clone(),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    );
    (
        Following {
            state,
            caller,
            id: agent.id.clone(),
            after: 5,
            turn: 3,
            pending: VecDeque::new(),
            follow: true,
            run: None,
            ended: false,
            watching,
            committed_message_seq: 4,
            held: None,
            authorization: None,
        },
        agent,
    )
}

fn fragment(agent: &Agent, after_seq: u64) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: agent.id.clone(),
        runtime_generation: 1,
        turn_sequence: 3,
        after_seq,
        message_id: "live-message".into(),
        kind: AgentLiveFragmentKind::Message,
        delta: "Live text".into(),
    }
}

#[tokio::test]
async fn live_frames_wait_for_the_captured_durable_high_water() {
    let (mut open, mut agent) = following().await;
    open.held = Some(fragment(&agent, 6).into());
    open.queue_live(&agent);
    assert!(open.pending.is_empty());
    assert!(open.held.is_some());
    agent.conversation.last_seq = 6;
    open.queue_live(&agent);
    assert!(open.held.is_none());
    let event = open.pending.pop_front().unwrap();
    assert_eq!(event.kind, "TEXT_MESSAGE_CONTENT");
    assert!(event.seq.is_none());
    let wire = serde_json::to_value(event).unwrap();
    assert_eq!(wire["cma:live"], true);
    assert!(wire.get("timestamp").is_none());
}

#[tokio::test]
async fn stale_generations_turns_and_superseded_text_are_never_projected() {
    let (mut open, agent) = following().await;
    let valid = fragment(&agent, 5);
    let mut stale = valid.clone();
    stale.runtime_generation = 2;
    let mut prior = valid.clone();
    prior.turn_sequence = 2;
    let mut foreign = valid.clone();
    foreign.agent_id = AgentId("agt_foreign".into());
    let mut superseded = valid.clone();
    superseded.after_seq = 3;
    for rejected in [stale, prior, foreign, superseded] {
        open.held = Some(rejected.into());
        open.queue_live(&agent);
        assert!(open.pending.is_empty());
    }
    open.held = Some(valid.into());
    open.queue_live(&agent);
    assert_eq!(open.pending.len(), 1);
    open.pending.clear();
    open.ended = true;
    open.held = Some(fragment(&agent, 5).into());
    open.queue_live(&agent);
    assert!(open.pending.is_empty());
}

#[tokio::test]
async fn authorized_snapshot_keeps_live_identity_generation_turn_and_commit_fences() {
    let (mut open, agent) = following().await;
    let valid = fragment(&agent, 5);
    for fault in [
        "foreign",
        "generation",
        "turn",
        "committed",
        "future",
        "valid",
    ] {
        let mut candidate = valid.clone();
        match fault {
            "foreign" => candidate.agent_id = AgentId("agt_foreign".into()),
            "generation" => candidate.runtime_generation += 1,
            "turn" => candidate.turn_sequence += 1,
            "committed" => candidate.after_seq = 3,
            "future" => candidate.after_seq = 6,
            _ => (),
        }
        open.held = Some(candidate.into());
        open.queue_authorized(layer2_core::sandbox::AgentEventStreamSnapshot {
            initial_model: agent.profile.content.model.clone(),
            created_at: "2026-09-05T00:00:00Z".into(),
            model: None,
            id: agent.id.clone(),
            runtime_generation: Some(1),
            active_turn: Some(3),
            head: 5,
            history: vec![],
        });
        if fault == "valid" {
            let event = open.pending.pop_front().unwrap();
            assert!(event.live && event.seq.is_none());
            assert_eq!(event.body["delta"], "Live text");
        } else {
            assert!(open.pending.is_empty());
            assert_eq!(open.held.is_some(), fault == "future");
        }
    }
}

fn approval(id: &str) -> AgentLogKind {
    AgentLogKind::ApprovalRequested {
        approval_id: id.into(),
        action: Some("read".into()),
        detail: Some("Read the workspace instructions".into()),
        runtime_locator: Some("private-runtime-locator".into()),
    }
}

#[tokio::test]
async fn resumed_run_finishes_with_approvals_recorded_before_its_cursor() {
    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_concurrent", AgentStatus::WaitingApproval, 9, Some(3));
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
                content: "Read the instructions".into(),
            },
            approval("first"),
            AgentLogKind::StatusChanged {
                from: AgentStatus::Working,
                to: AgentStatus::WaitingApproval,
            },
            approval("second"),
            approval("third"),
            AgentLogKind::ApprovalResolved {
                approval_id: "first".into(),
                decision: "allow".into(),
            },
        ],
    )
    .await;
    let caller = Caller::new(
        agent.owner.clone(),
        layer2_core::Authorization::new([layer2_core::Capability::UseOwnSandboxes]),
    );
    let response = open(state, caller, agent.id, 8, true, Some(3), None).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = tokio::time::timeout(
        Duration::from_secs(1),
        axum::body::to_bytes(response.into_body(), usize::MAX),
    )
    .await
    .expect("the resume stream must release the next approval button")
    .unwrap();
    let wire = String::from_utf8(bytes.to_vec()).unwrap();
    let frames: Vec<serde_json::Value> = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(frames.first().unwrap()["type"], "RUN_STARTED");
    let finished: Vec<_> = frames
        .iter()
        .filter(|frame| frame["type"] == "RUN_FINISHED")
        .collect();
    assert_eq!(finished.len(), 1);
    assert_eq!(finished[0]["outcome"]["type"], "interrupt");
    let interrupts = finished[0]["outcome"]["interrupts"].as_array().unwrap();
    assert_eq!(interrupts.len(), 2);
    assert_eq!(interrupts[0]["id"], "second");
    assert_eq!(interrupts[1]["id"], "third");
    assert!(!wire.contains("private-runtime-locator"));
    assert!(
        wire.trim_end().ends_with("id: 9"),
        "the terminal snapshot must complete the SDK projection at the current head"
    );
    assert_eq!(
        wire.lines()
            .filter(|line| line.starts_with("id:"))
            .collect::<Vec<_>>(),
        vec!["id: 9"]
    );
}

#[tokio::test]
async fn resolved_approvals_do_not_interrupt_work_or_a_continuous_replay() {
    let (mut open, mut agent) = following().await;
    let mut history = vec![AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: 5,
        appended_at: "2026-09-07T00:00:00Z".into(),
        kind: approval("first"),
    }];
    agent.status = AgentStatus::WaitingApproval;
    open.interrupt_pending(&agent, &history);
    assert!(!open.ended, "continuous replay must keep following");
    assert!(open.pending.is_empty());
    open.run = Some(3);
    agent.status = AgentStatus::Working;
    open.interrupt_pending(&agent, &history);
    assert!(!open.ended);
    history.push(AgentLogEntry {
        seq: 6,
        kind: AgentLogKind::ApprovalResolved {
            approval_id: "first".into(),
            decision: "allow".into(),
        },
        ..history[0].clone()
    });
    agent.status = AgentStatus::WaitingApproval;
    open.interrupt_pending(&agent, &history);
    assert!(!open.ended, "resolved requests must not be reintroduced");
    assert!(open.pending.is_empty());
}
