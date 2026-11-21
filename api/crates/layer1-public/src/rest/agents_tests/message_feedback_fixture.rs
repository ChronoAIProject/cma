use std::collections::BTreeMap;

use layer2_core::sandbox::message_feedback::{
    FeedbackDetails, FeedbackReasonError, FeedbackReasonRequest, MessageFeedback,
    MessageFeedbackStore, MessageVote,
};
use layer2_core::sandbox::AgentReadError;

use super::super::*;

type FeedbackKey = (SubjectId, AgentId, u64);
#[derive(Default)]
pub(in crate::rest) struct FeedbackMemory(Mutex<BTreeMap<FeedbackKey, FeedbackDetails>>);
#[async_trait]
impl MessageFeedbackStore for FeedbackMemory {
    async fn get(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
    ) -> Result<FeedbackDetails, AgentReadError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(&(owner.clone(), agent.clone(), sequence))
            .cloned()
            .unwrap_or_default())
    }
    async fn set_reasons(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        request: &FeedbackReasonRequest,
    ) -> Result<FeedbackDetails, FeedbackReasonError> {
        let mut rows = self.0.lock().unwrap();
        let current = rows
            .entry((owner.clone(), agent.clone(), sequence))
            .or_default();
        *current = current.with_reasons(request)?;
        Ok(current.clone())
    }
    async fn list(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<MessageFeedback>, AgentReadError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|((subject, agent_id, sequence), state)| {
                subject == owner
                    && agent_id == agent
                    && *sequence > after
                    && *sequence <= through
                    && state.vote.is_some()
            })
            .take(usize::from(limit))
            .map(|((_, _, sequence), state)| MessageFeedback {
                sequence: *sequence,
                vote: state.vote.unwrap(),
            })
            .collect())
    }
    async fn set(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        vote: Option<MessageVote>,
    ) -> Result<(), AgentReadError> {
        let mut rows = self.0.lock().unwrap();
        let key = (owner.clone(), agent.clone(), sequence);
        let current = rows.entry(key).or_default();
        *current = current.with_vote(vote)?;
        Ok(())
    }
}

pub(in crate::rest) async fn feedback_fixture() -> (Fixture, Arc<FeedbackMemory>, Agent) {
    let mut fixture = fixture().await;
    let feedback = Arc::new(FeedbackMemory::default());
    fixture.state = fixture.state.with_message_feedback(feedback.clone());
    fixture.router = agent_router(fixture.state.clone());
    let agent = insert_agent(
        &fixture.store,
        seeded_agent("agt_feedback", AgentStatus::Ready, 10, None),
        vec![
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 3,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Question".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 3 },
            AgentLogKind::TurnStarted {
                model: None,
                turn_sequence: 7,
                origin_key: None,
            },
            AgentLogKind::OwnerMessage {
                content: "Try again".into(),
            },
            AgentLogKind::AgentMessage {
                content: "Answer".into(),
            },
            AgentLogKind::TurnCompleted { turn_sequence: 7 },
        ],
    )
    .await;
    (fixture, feedback, agent)
}
