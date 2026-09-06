use super::*;
use crate::rest::agents_tests::{conversation_fixture, seeded_agent};
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
    open.held = Some(fragment(&agent, 6));
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
        open.held = Some(rejected);
        open.queue_live(&agent);
        assert!(open.pending.is_empty());
    }
    open.held = Some(valid);
    open.queue_live(&agent);
    assert_eq!(open.pending.len(), 1);
    open.pending.clear();
    open.ended = true;
    open.held = Some(fragment(&agent, 5));
    open.queue_live(&agent);
    assert!(open.pending.is_empty());
}
