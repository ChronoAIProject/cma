use super::{Agents, MessageVote};
use crate::{Error, Result};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};

#[doc = "A reason attached to a confirmed negative answer vote."]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackReason {
    #[doc = "Content is inaccurate."]
    Inaccurate,
    #[doc = "The question was not answered."]
    Unanswered,
    #[doc = "The proposed steps do not work."]
    UnusableSteps,
    #[doc = "The answer is incomplete."]
    Incomplete,
    #[doc = "The wording is unclear."]
    Unclear,
    #[doc = "Another reason."]
    Other,
}

#[doc = "An exact conditional reason write; retain this body after an uncertain response."]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackReasonRequest {
    #[doc = "Version returned by the public feedback read."]
    pub expected_version: u64,
    #[doc = "Stable ASCII request identity of 1 to 128 characters."]
    pub request_id: String,
    #[doc = "Zero to six distinct reasons."]
    pub reasons: Vec<FeedbackReason>,
    #[doc = "Optional description; empty or at most 500 Unicode code points."]
    #[serde(default)]
    pub comment: String,
}

#[doc = "Current private feedback, including the version needed to save reasons."]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FeedbackDetails {
    #[doc = "Authenticated owner identity."]
    pub viewer_id: String,
    #[doc = "Agent identity."]
    pub agent_id: String,
    #[doc = "Canonical durable answer identity."]
    pub message_id: String,
    #[doc = "Confirmed vote; null means removed."]
    pub vote: Option<MessageVote>,
    #[doc = "Monotonic version, retained across removal."]
    pub version: u64,
    #[doc = "Saved reasons; empty for historical vote-only records."]
    pub reasons: Vec<FeedbackReason>,
    #[doc = "Saved optional description."]
    pub comment: String,
    #[doc = "Last change timestamp; absent for untouched historical records."]
    pub updated_at: Option<String>,
    #[doc = "Last accepted reason request, for uncertain-response recovery."]
    pub request_id: Option<String>,
}

impl Agents<'_> {
    #[doc = "Read the owner's current vote and reason version without changing Agent work."]
    pub async fn feedback_details(&self, id: &str, message_id: &str) -> Result<FeedbackDetails> {
        validate_identity(message_id)?;
        let result = self
            .client
            .get(&["agents", id, "messages", message_id, "feedback"], &[])
            .await?;
        validate_receipt(&result, id, message_id)?;
        Ok(result)
    }

    #[doc = "Conditionally save reasons. Repeat the identical body to recover an uncertain result."]
    pub async fn set_feedback_reasons(
        &self,
        id: &str,
        message_id: &str,
        body: &FeedbackReasonRequest,
    ) -> Result<FeedbackDetails> {
        validate_identity(message_id)?;
        if body.comment.chars().count() > 500
            || body.request_id.is_empty()
            || body.request_id.len() > 128
            || !body
                .request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_.:-".contains(&byte))
            || body.expected_version > 9_007_199_254_740_990
            || body.reasons.len() > 6
            || body
                .reasons
                .iter()
                .enumerate()
                .any(|(index, reason)| body.reasons[..index].contains(reason))
        {
            return Err(Error::validation(
                "invalid feedback reasons, comment or request identity",
            ));
        }
        let result: FeedbackDetails = self
            .client
            .operation(
                Method::PUT,
                &["agents", id, "messages", message_id, "feedback", "reasons"],
                Some(body),
                StatusCode::OK,
            )
            .await?;
        validate_receipt(&result, id, message_id)?;
        if result.vote != Some(MessageVote::Down)
            || result.version != body.expected_version + 1
            || result.request_id.as_deref() != Some(&body.request_id)
            || result.reasons != body.reasons
            || result.comment != body.comment
        {
            return Err(Error::protocol(
                "feedback reasons receipt differs from the request",
            ));
        }
        Ok(result)
    }
}

fn validate_identity(message_id: &str) -> Result<()> {
    if !message_id
        .strip_prefix("event:")
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|value| value > 0 && message_id == format!("event:{value}"))
    {
        return Err(Error::validation(
            "expected event:<positive sequence> message ID",
        ));
    }
    Ok(())
}

fn validate_receipt(result: &FeedbackDetails, agent: &str, message: &str) -> Result<()> {
    crate::resources::identity(&result.agent_id, agent)?;
    crate::resources::identity(&result.message_id, message)?;
    if result.comment.chars().count() > 500
        || result.reasons.len() > 6
        || result
            .reasons
            .iter()
            .enumerate()
            .any(|(index, reason)| result.reasons[..index].contains(reason))
        || (result.vote != Some(MessageVote::Down)
            && (!result.reasons.is_empty() || !result.comment.is_empty()))
    {
        return Err(Error::protocol("invalid feedback reasons receipt"));
    }
    Ok(())
}
