use serde::{Deserialize, Serialize};

use super::{AgentReadError, MessageVote};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackReason {
    Inaccurate,
    Unanswered,
    UnusableSteps,
    Incomplete,
    Unclear,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackReasonRequest {
    pub expected_version: u64,
    pub request_id: String,
    pub reasons: Vec<FeedbackReason>,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeedbackDetails {
    pub vote: Option<MessageVote>,
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub reasons: Vec<FeedbackReason>,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub last_request: Option<FeedbackReasonRequest>,
}

#[derive(Debug, thiserror::Error)]
pub enum FeedbackReasonError {
    #[error(transparent)]
    Read(#[from] AgentReadError),
    #[error("feedback changed; read the current vote before saving reasons")]
    Conflict,
    #[error("feedback reasons are invalid")]
    Invalid,
}

impl FeedbackDetails {
    pub fn with_vote(&self, vote: Option<MessageVote>) -> Result<Self, AgentReadError> {
        if self.vote == vote {
            return Ok(self.clone());
        }
        Ok(Self {
            vote,
            version: self.next_version()?,
            updated_at: Some(timestamp()?),
            ..Self::default()
        })
    }

    pub fn with_reasons(
        &self,
        request: &FeedbackReasonRequest,
    ) -> Result<Self, FeedbackReasonError> {
        if request.comment.chars().count() > 500
            || request.request_id.is_empty()
            || request.request_id.len() > 128
            || !request
                .request_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
            || request.reasons.len() > 6
            || request
                .reasons
                .iter()
                .enumerate()
                .any(|(index, reason)| request.reasons[..index].contains(reason))
        {
            return Err(FeedbackReasonError::Invalid);
        }
        if self.vote != Some(MessageVote::Down) {
            return Err(FeedbackReasonError::Conflict);
        }
        if self.last_request.as_ref() == Some(request)
            && request.expected_version.checked_add(1) == Some(self.version)
        {
            return Ok(self.clone());
        }
        if self.version != request.expected_version
            || self
                .last_request
                .as_ref()
                .is_some_and(|previous| previous.request_id == request.request_id)
        {
            return Err(FeedbackReasonError::Conflict);
        }
        Ok(Self {
            vote: self.vote,
            version: self.next_version()?,
            reasons: request.reasons.clone(),
            comment: request.comment.clone(),
            updated_at: Some(timestamp()?),
            last_request: Some(request.clone()),
        })
    }

    fn next_version(&self) -> Result<u64, AgentReadError> {
        self.version
            .checked_add(1)
            .filter(|value| *value <= 9_007_199_254_740_991)
            .ok_or_else(|| AgentReadError::Corrupt("feedback version exhausted".into()))
    }
}

fn timestamp() -> Result<String, AgentReadError> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| AgentReadError::Unavailable("feedback clock unavailable".into()))
}
