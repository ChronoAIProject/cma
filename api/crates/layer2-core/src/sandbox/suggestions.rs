use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{agent_io::AnswerContext, AgentReadError, AgentReadService};
use crate::{AgentId, Caller, SubjectId};

mod context;
mod generation;
mod quality;
pub use context::work_is_current;
pub use context::{authority_digest, work_from_turn, SuggestionContext, MAX_CONTEXT_EVENTS};
pub use generation::*;

pub const STRATEGY_VERSION: &str = "contextual-v3";
pub const GENERATION_TIMEOUT_SECONDS: u64 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SuggestedReplySet {
    pub id: String,
    pub viewer_id: String,
    pub agent_id: String,
    pub answer_id: String,
    pub scope: AnswerContext,
    pub source: SuggestionSource,
    pub items: Vec<SuggestedReply>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionSource {
    pub kind: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestedReply {
    pub id: String,
    pub text: String,
    pub rank: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionStatus {
    Pending,
    Running,
    Ready,
    Empty,
    Failed,
    Expired,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionReading {
    pub status: SuggestionStatus,
    pub set: Option<SuggestedReplySet>,
    pub observation: Option<SuggestionObservation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionWork {
    pub id: String,
    pub owner: SubjectId,
    pub agent_id: AgentId,
    pub scope: AnswerContext,
    pub completed_sequence: u64,
    pub strategy_version: String,
    pub authority: String,
    pub status: SuggestionStatus,
    pub context: SuggestionContext,
    pub set: Option<SuggestedReplySet>,
    pub created_at_ms: u64,
    pub started_at_ms: Option<u64>,
    pub deadline_ms: Option<u64>,
    pub observation: Option<SuggestionObservation>,
}

#[async_trait]
pub trait SuggestionStore: Send + Sync {
    async fn read_suggestions(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        scope: AnswerContext,
    ) -> Result<SuggestionReading, AgentReadError>;
    async fn claim_suggestions(
        &self,
        now_ms: u64,
    ) -> Result<Option<SuggestionWork>, AgentReadError>;
    async fn complete_suggestions(
        &self,
        work: &SuggestionWork,
        completion: SuggestionCompletion,
    ) -> Result<(), AgentReadError>;
}

impl SuggestionWork {
    pub fn reading(&self, current: bool) -> SuggestionReading {
        // Deadline settlement belongs to the claim path. A still-running
        // claim can publish a result, so a read must keep it observable.
        let status = if current {
            self.status
        } else {
            SuggestionStatus::Expired
        };
        SuggestionReading {
            status,
            set: (status == SuggestionStatus::Ready)
                .then(|| self.set.clone())
                .flatten(),
            observation: self.observation.clone(),
        }
    }

    pub fn claim(&mut self, current: bool, now_ms: u64) -> bool {
        if self.status == SuggestionStatus::Running
            && self.deadline_ms.is_some_and(|deadline| deadline <= now_ms)
        {
            self.status = SuggestionStatus::Failed;
            self.observation = Some(SuggestionObservation {
                elapsed_ms: now_ms.saturating_sub(self.started_at_ms.unwrap_or(now_ms)),
                failure: Some(SuggestionFailure::Interrupted),
                usage: SuggestionUsage::default(),
            });
        }
        if self.status != SuggestionStatus::Pending {
            return false;
        }
        if !current
            || self.strategy_version != STRATEGY_VERSION
            || now_ms.saturating_sub(self.created_at_ms) > 120_000
        {
            self.status = SuggestionStatus::Expired;
            return false;
        }
        self.status = SuggestionStatus::Running;
        self.started_at_ms = Some(now_ms);
        self.deadline_ms = Some(now_ms + (GENERATION_TIMEOUT_SECONDS + 10) * 1000);
        true
    }

    pub fn complete(&mut self, current: bool, completion: SuggestionCompletion) {
        self.observation = Some(completion.observation);
        self.status = if !current {
            SuggestionStatus::Expired
        } else {
            completion.status
        };
        self.set = (self.status == SuggestionStatus::Ready)
            .then_some(completion.set)
            .flatten();
    }
}

pub struct SuggestionService {
    reads: Arc<AgentReadService>,
    store: Arc<dyn SuggestionStore>,
}

impl SuggestionService {
    pub fn new(reads: Arc<AgentReadService>, store: Arc<dyn SuggestionStore>) -> Arc<Self> {
        Arc::new(Self { reads, store })
    }

    pub async fn read(
        &self,
        caller: &Caller,
        agent: &AgentId,
        scope: AnswerContext,
    ) -> Result<SuggestionReading, AgentReadError> {
        scope.validate().map_err(|_| AgentReadError::NotFound)?;
        self.reads.read(caller, agent).await?;
        self.store
            .read_suggestions(&caller.subject, agent, scope)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_suggestion_read_stays_running_until_claim_settlement() {
        let mut work = SuggestionWork {
            id: "suggestion".into(),
            owner: SubjectId::new("owner"),
            agent_id: AgentId("agent".into()),
            scope: AnswerContext {
                turn_sequence: 1,
                answer_sequence: 2,
            },
            completed_sequence: 3,
            strategy_version: STRATEGY_VERSION.into(),
            authority: "authority".into(),
            status: SuggestionStatus::Pending,
            context: SuggestionContext {
                goal: "Compare options".into(),
                answer: "A comparison".into(),
                confirmed_resources: Vec::new(),
            },
            set: None,
            created_at_ms: 0,
            started_at_ms: None,
            deadline_ms: None,
            observation: None,
        };

        assert!(work.claim(true, 0));
        let deadline = work.deadline_ms.unwrap();
        assert_eq!(work.reading(true).status, SuggestionStatus::Running);
        assert_eq!(work.reading(false).status, SuggestionStatus::Expired);
        assert!(!work.claim(true, deadline));
        let settled = work.reading(true);
        assert_eq!(settled.status, SuggestionStatus::Failed);
        assert_eq!(
            settled.observation.unwrap().failure,
            Some(SuggestionFailure::Interrupted)
        );
    }
}
