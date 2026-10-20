//! Owner-only message metadata; feedback never sends input to an Agent.
use super::Agents;
use crate::{Error, EventOptions, ResourcePage, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

/// A single owner's positive or negative feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageVote {
    /// Positive feedback.
    Up,
    /// Negative feedback.
    Down,
}
/// Current feedback on one durable message.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MessageFeedback {
    /// `event:<sequence>` from the Agent transcript.
    pub message_id: String,
    /// Null means this owner's vote was removed.
    pub vote: Option<MessageVote>,
}
#[derive(Serialize)]
struct FeedbackRequest {
    vote: Option<MessageVote>,
}
impl Agents<'_> {
    /// List saved votes. Continue with the returned `next` sequence as `after`.
    pub async fn message_feedback(
        &self,
        id: &str,
        options: EventOptions,
    ) -> Result<ResourcePage<MessageFeedback, u64>> {
        self.client
            .get(&["agents", id, "message-feedback"], &options.query())
            .await
    }
    /// Idempotently replace or clear a vote, without a command or runtime effect.
    pub async fn set_message_feedback(
        &self,
        id: &str,
        message_id: &str,
        vote: Option<MessageVote>,
    ) -> Result<MessageFeedback> {
        if !valid_message_id(message_id) {
            return Err(Error::validation(
                "expected event:<positive sequence> message ID",
            ));
        }
        let result: MessageFeedback = self
            .client
            .operation(
                Method::PUT,
                &["agents", id, "messages", message_id, "feedback"],
                Some(&FeedbackRequest { vote }),
                StatusCode::OK,
            )
            .await?;
        crate::resources::identity(&result.message_id, message_id)?;
        Ok(result)
    }
}
fn valid_message_id(value: &str) -> bool {
    value
        .strip_prefix("event:")
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .is_some_and(|sequence| sequence > 0 && value == format!("event:{sequence}"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn message_feedback_requires_canonical_durable_identity_and_serializes_null() {
        assert!(valid_message_id("event:5"));
        for id in ["event:0", "event:05", "optimistic:5", "event:-1"] {
            assert!(!valid_message_id(id));
        }
        assert_eq!(
            serde_json::to_value(FeedbackRequest { vote: None }).unwrap(),
            serde_json::json!({"vote":null})
        );
    }
}
