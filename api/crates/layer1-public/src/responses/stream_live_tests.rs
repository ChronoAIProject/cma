use super::*;
use crate::responses::{retrieve, OutputMessage, OutputText};
use crate::rest::agents_tests::{conversation_fixture, seeded_agent};
use layer2_core::sandbox::{AgentStatus, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use layer2_core::{Authorization, Caller, Capability};

async fn opening() -> (Open, Agent) {
    let (state, store) = conversation_fixture().await;
    let agent = seeded_agent("agt_live", AgentStatus::Working, 5, Some(3));
    let recorder = DurableAgentUpdateRecorder::new(store);
    let watching = recorder.subscribe_live(&agent.id);
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    (
        Open {
            state,
            caller,
            agent: agent.clone(),
            turn: 3,
            cursor: 5,
            sequence: 0,
            pending: Default::default(),
            tools: Default::default(),
            announced: true,
            output_count: 0,
            next_output_index: 0,
            ended: false,
            live: LiveState::new(watching),
            authorization: None,
        },
        agent,
    )
}

fn fragment(agent: &Agent, after_seq: u64, kind: AgentLiveFragmentKind) -> AgentLiveFragment {
    AgentLiveFragment {
        agent_id: agent.id.clone(),
        runtime_generation: 1,
        turn_sequence: 3,
        after_seq,
        message_id: "live-message".into(),
        kind,
        delta: "Live text".into(),
    }
}

fn response(agent: &Agent, text: &str) -> crate::responses::ResponseObject {
    let mut response = retrieve::empty(&crate::agent_page::AgentPageOrigin::test(), agent, 3);
    response.output.push(OutputMessage {
        id: retrieve::output_id(&agent.id, 3, 0),
        kind: "message",
        status: "completed",
        role: "assistant",
        content: vec![OutputText {
            kind: "output_text",
            text: text.into(),
            annotations: vec![],
            logprobs: vec![],
        }],
    });
    response
}

#[tokio::test]
async fn live_text_waits_for_durable_high_water_and_reconciles_without_duplicate_prefix() {
    let (mut open, mut agent) = opening().await;
    open.live.held = Some(fragment(&agent, 6, AgentLiveFragmentKind::Message));
    open.drain_live(&agent, 4);
    assert!(open.pending.is_empty());
    assert!(open.live.held.is_some());
    agent.conversation.last_seq = 6;
    open.drain_live(&agent, 4);
    assert!(open.live.held.is_none());
    assert_eq!(open.pending.len(), 3);
    let projected = response(&agent, "Live text completed");
    open.append_output(&projected, 1);
    let deltas: Vec<_> = open
        .pending
        .iter()
        .filter(|frame| frame["type"] == "response.output_text.delta")
        .map(|frame| frame["delta"].as_str().unwrap())
        .collect();
    assert_eq!(deltas, ["Live text", " completed"]);
    assert_eq!(
        open.pending
            .iter()
            .filter(|frame| frame["type"] == "response.output_item.added")
            .count(),
        1
    );
    assert_eq!(
        open.pending.back().unwrap()["item"],
        serde_json::to_value(&projected.output[0]).unwrap()
    );
    assert!(open.live.message_text.is_empty());
}

#[tokio::test]
async fn foreign_stale_and_superseded_fragments_are_discarded() {
    let (mut open, agent) = opening().await;
    let valid = fragment(&agent, 5, AgentLiveFragmentKind::Message);
    let mut stale = valid.clone();
    stale.runtime_generation = 2;
    let mut prior = valid.clone();
    prior.turn_sequence = 2;
    let mut foreign = valid.clone();
    foreign.agent_id = layer2_core::AgentId("agt_other".into());
    let mut superseded = valid;
    superseded.after_seq = 3;
    for rejected in [stale, prior, foreign, superseded] {
        open.live.held = Some(rejected);
        open.drain_live(&agent, 4);
        assert!(open.pending.is_empty());
    }
    let mut terminal = agent.clone();
    terminal.conversation.active_turn = None;
    open.live.held = Some(fragment(&agent, 5, AgentLiveFragmentKind::Thought));
    open.drain_live(&terminal, 4);
    assert!(open.pending.is_empty());
}

#[tokio::test]
async fn authoritative_completion_replaces_incomplete_or_different_live_text() {
    let (mut open, agent) = opening().await;
    open.message_delta("missed-prefix");
    let projected = response(&agent, "The durable answer");
    open.append_output(&projected, 1);
    let done = open
        .pending
        .iter()
        .find(|frame| frame["type"] == "response.output_text.done")
        .unwrap();
    assert_eq!(done["text"], "The durable answer");
    assert_eq!(
        open.pending.back().unwrap()["item"],
        serde_json::to_value(&projected.output[0]).unwrap()
    );
}

#[tokio::test]
async fn live_assembly_is_utf8_bounded_and_reasoning_remains_ephemeral() {
    let (mut open, agent) = opening().await;
    let text = "\u{20ac}".repeat(MAX_LIVE_REASONING_BYTES);
    open.thought_delta(&text);
    open.thought_delta(&text);
    assert!(open.live.reasoning_text.len() <= MAX_LIVE_REASONING_BYTES);
    assert!(open.live.reasoning_truncated);
    open.finish_reasoning();
    let done = open
        .pending
        .iter()
        .find(|frame| frame["type"] == "response.reasoning_summary_text.done")
        .unwrap();
    assert_eq!(done["cma:truncated"], true);
    assert!(open.live.reasoning_text.is_empty());
    assert!(
        retrieve::empty(&crate::agent_page::AgentPageOrigin::test(), &agent, 3)
            .output
            .is_empty()
    );
    open.message_delta(&"x".repeat(MAX_DURABLE_AGENT_MESSAGE_BYTES + 8));
    assert_eq!(
        open.live.message_text.len(),
        MAX_DURABLE_AGENT_MESSAGE_BYTES
    );
    let count = open.pending.len();
    open.message_delta("more");
    assert_eq!(open.pending.len(), count);
}
