//! Owner feedback is metadata on a durable message, never Agent input.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{AgentId, Caller, SubjectId};

use super::{AgentReadError, AgentReadService};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageVote {
    Up,
    Down,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFeedback {
    pub sequence: u64,
    pub vote: MessageVote,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFeedbackPage {
    pub items: Vec<MessageFeedback>,
    pub next: Option<u64>,
}

/// Records are scoped by the authenticated owner and immutable Agent/message IDs.
/// Implementations must not alter Agent state, logs, commands or runtime work.
#[async_trait]
pub trait MessageFeedbackStore: Send + Sync {
    async fn list(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        after: u64,
        through: u64,
        limit: u16,
    ) -> Result<Vec<MessageFeedback>, AgentReadError>;

    /// Replaces the current vote, or removes it for `None`. Repeating a request
    /// has the same effect; concurrent independent requests are last-write-wins.
    async fn set(
        &self,
        owner: &SubjectId,
        agent: &AgentId,
        sequence: u64,
        vote: Option<MessageVote>,
    ) -> Result<(), AgentReadError>;
}

pub struct MessageFeedbackService {
    reads: Arc<AgentReadService>,
    store: Arc<dyn MessageFeedbackStore>,
}

impl MessageFeedbackService {
    pub fn new(reads: Arc<AgentReadService>, store: Arc<dyn MessageFeedbackStore>) -> Arc<Self> {
        Arc::new(Self { reads, store })
    }

    pub async fn list(
        &self,
        caller: &Caller,
        agent: &AgentId,
        after: u64,
        limit: u16,
    ) -> Result<MessageFeedbackPage, AgentReadError> {
        let current = self.reads.read(caller, agent).await?;
        if limit == 0 || limit > 200 {
            return Err(AgentReadError::InvalidPage);
        }
        let mut items = self
            .store
            .list(
                &caller.subject,
                agent,
                after,
                current.conversation.last_seq,
                limit + 1,
            )
            .await?;
        if items.len() > usize::from(limit + 1)
            || items
                .iter()
                .any(|item| item.sequence > current.conversation.last_seq)
            || items.first().is_some_and(|item| item.sequence <= after)
            || items
                .windows(2)
                .any(|pair| pair[0].sequence >= pair[1].sequence)
        {
            return Err(AgentReadError::Corrupt(
                "invalid message feedback page".into(),
            ));
        }
        let next = if items.len() > usize::from(limit) {
            items.pop();
            items.last().map(|item| item.sequence)
        } else {
            None
        };
        Ok(MessageFeedbackPage { items, next })
    }

    pub async fn set(
        &self,
        caller: &Caller,
        agent: &AgentId,
        sequence: u64,
        vote: Option<MessageVote>,
    ) -> Result<(), AgentReadError> {
        self.reads
            .require_owned_message(caller, agent, sequence)
            .await?;
        self.store.set(&caller.subject, agent, sequence, vote).await
    }
}
