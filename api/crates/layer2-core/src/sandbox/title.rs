//! Privacy-preserving values at the agent-title boundary.

use std::fmt;

use async_trait::async_trait;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use super::title_sanitize::sanitized_title;

pub const AGENT_TITLE_SYSTEM_PROMPT: &str =
    "Create a 3-6 word conversation title. Return only the title, with no quotes or ornament.";
pub const AGENT_TITLE_INPUT_BYTES: usize = 2 * 1024;
pub const AGENT_TITLE_MAX_SCALARS: usize = 64;
pub const AGENT_TITLE_FALLBACK_SCALARS: usize = 48;
pub const AGENT_TITLE_CALL_TIMEOUT_SECONDS: u64 = 10;
pub const AGENT_TITLE_CLAIM_SECONDS: u64 = 45;
pub const AGENT_TITLE_ATTEMPTS: usize = 2;
pub const EMPTY_AGENT_TITLE_FALLBACK: &str = "New conversation";

/// Why the write-once title was selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTitleSource {
    User,
    Generated,
    Fallback,
}

impl AgentTitleSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Generated => "generated",
            Self::Fallback => "fallback",
        }
    }
}

/// Why a string cannot be represented as a canonical title.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentTitleError {
    #[error("agent title is empty")]
    Empty,
    #[error("agent title exceeds 64 Unicode scalars")]
    TooLong,
    #[error("agent title contains control characters")]
    ContainsControl,
    #[error("agent title is not in sanitized first-line form")]
    NotSanitized,
}

/// One non-empty, sanitized title of at most 64 Unicode scalars.
///
/// Debug deliberately reveals neither user-entered nor model-generated text.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentTitle(String);

impl AgentTitle {
    /// Validate an already-sanitized title without changing it.
    pub fn new(value: impl Into<String>) -> Result<Self, AgentTitleError> {
        let value = value.into();
        if value.is_empty() {
            return Err(AgentTitleError::Empty);
        }
        if value.chars().count() > AGENT_TITLE_MAX_SCALARS {
            return Err(AgentTitleError::TooLong);
        }
        if value.chars().any(char::is_control) {
            return Err(AgentTitleError::ContainsControl);
        }
        if sanitized_title(&value, AGENT_TITLE_MAX_SCALARS).as_deref() != Some(value.as_str()) {
            return Err(AgentTitleError::NotSanitized);
        }
        Ok(Self(value))
    }

    /// Sanitize one untrusted user or generator value into the canonical form.
    pub fn sanitize(value: &str) -> Option<Self> {
        let value = sanitized_title(value, AGENT_TITLE_MAX_SCALARS)?;
        Self::new(value).ok()
    }

    /// Derive the bounded deterministic fallback from the first owner message.
    pub fn fallback(owner_message: Option<&str>) -> Self {
        owner_message
            .and_then(|value| sanitized_title(value, AGENT_TITLE_FALLBACK_SCALARS))
            .and_then(|value| Self::new(value).ok())
            .unwrap_or_else(|| {
                Self::new(EMPTY_AGENT_TITLE_FALLBACK)
                    .expect("the static fallback is a canonical agent title")
            })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentTitle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentTitle(<redacted>)")
    }
}

impl Serialize for AgentTitle {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AgentTitle {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

/// Authorized model selection and credential for one title attempt.
#[derive(Clone)]
pub struct AgentTitleGenerationContext {
    model: String,
    bearer: String,
}

impl AgentTitleGenerationContext {
    pub fn new(model: impl Into<String>, bearer: impl Into<String>) -> Option<Self> {
        let model = model.into();
        let bearer = bearer.into();
        (!model.trim().is_empty() && !bearer.trim().is_empty()).then_some(Self { model, bearer })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn bearer(&self) -> &str {
        &self.bearer
    }
}

impl fmt::Debug for AgentTitleGenerationContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentTitleGenerationContext")
            .field("model", &self.model)
            .field(
                "bearer",
                &format_args!("<redacted {} bytes>", self.bearer.len()),
            )
            .finish()
    }
}

/// Bounded first-turn evidence. Conversation content is always redacted in Debug.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentTitleTranscript {
    turn_sequence: u64,
    owner_message: String,
    assistant_message: String,
}

impl AgentTitleTranscript {
    pub fn from_agent_log(agent: &super::Agent, logs: &[super::AgentLogEntry]) -> Option<Self> {
        let first = agent.conversation.first_message.as_ref();
        if first.is_none_or(|message| message.purpose == Some(super::FirstMessagePurpose::Task)) {
            return Self::from_complete_log(logs);
        }
        let setup_turn = first?.turn_sequence?;
        let start = logs.iter().position(|entry| {
            matches!(
                entry.kind,
                super::AgentLogKind::TurnStarted { turn_sequence, .. } if turn_sequence > setup_turn
            )
        })?;
        Self::from_complete_log(&logs[start..])
    }

    /// Fold a complete, gap-free agent log into the exact first completed turn.
    ///
    /// Durable adapters call this only after proving log sequence integrity.
    #[doc(hidden)]
    pub fn from_complete_log(logs: &[super::AgentLogEntry]) -> Option<Self> {
        let mut turn_sequence = None;
        let mut owner = None;
        let mut assistant = None;

        for entry in logs {
            match &entry.kind {
                super::AgentLogKind::TurnStarted {
                    turn_sequence: sequence,
                    ..
                } if turn_sequence.is_none() && *sequence > 0 => turn_sequence = Some(*sequence),
                super::AgentLogKind::TurnStarted { .. } => return None,
                super::AgentLogKind::OwnerMessage { content }
                    if turn_sequence.is_some() && owner.is_none() && assistant.is_none() =>
                {
                    owner = Some(content.as_str());
                }
                super::AgentLogKind::AgentMessage { content }
                    if turn_sequence.is_some() && owner.is_some() =>
                {
                    assistant = Some(content.as_str());
                }
                super::AgentLogKind::TurnCompleted {
                    turn_sequence: completed,
                } if Some(*completed) == turn_sequence
                    && owner.is_some()
                    && assistant.is_some() =>
                {
                    return Some(Self::completed(
                        *completed,
                        owner.expect("checked"),
                        assistant.expect("checked"),
                    ));
                }
                super::AgentLogKind::OwnerMessage { .. }
                | super::AgentLogKind::AgentMessage { .. }
                | super::AgentLogKind::TurnCompleted { .. }
                | super::AgentLogKind::TurnFailed { .. }
                | super::AgentLogKind::TurnInterrupted { .. } => return None,
                _ => {}
            }
        }
        None
    }

    pub(crate) fn completed(
        turn_sequence: u64,
        owner_message: &str,
        assistant_message: &str,
    ) -> Self {
        Self {
            turn_sequence,
            owner_message: truncate_utf8_bytes(owner_message, AGENT_TITLE_INPUT_BYTES),
            assistant_message: truncate_utf8_bytes(assistant_message, AGENT_TITLE_INPUT_BYTES),
        }
    }

    pub fn turn_sequence(&self) -> u64 {
        self.turn_sequence
    }

    pub fn owner_message(&self) -> &str {
        &self.owner_message
    }

    pub fn assistant_message(&self) -> &str {
        &self.assistant_message
    }
}

impl fmt::Debug for AgentTitleTranscript {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentTitleTranscript")
            .field("turn_sequence", &self.turn_sequence)
            .field(
                "owner_message",
                &format_args!("<redacted {} bytes>", self.owner_message.len()),
            )
            .field(
                "assistant_message",
                &format_args!("<redacted {} bytes>", self.assistant_message.len()),
            )
            .finish()
    }
}

/// Private inputs passed to the narrow runtime-facing generator.
#[derive(Clone)]
pub struct AgentTitleGenerationRequest {
    model: String,
    owner_message: String,
    assistant_message: String,
    bearer: String,
}

impl AgentTitleGenerationRequest {
    pub(crate) fn new(
        context: &AgentTitleGenerationContext,
        transcript: &AgentTitleTranscript,
    ) -> Self {
        Self {
            model: context.model.clone(),
            owner_message: truncate_utf8_bytes(transcript.owner_message(), AGENT_TITLE_INPUT_BYTES),
            assistant_message: truncate_utf8_bytes(
                transcript.assistant_message(),
                AGENT_TITLE_INPUT_BYTES,
            ),
            bearer: context.bearer.clone(),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub const fn system_prompt(&self) -> &'static str {
        AGENT_TITLE_SYSTEM_PROMPT
    }

    pub fn owner_message(&self) -> &str {
        &self.owner_message
    }

    pub fn assistant_message(&self) -> &str {
        &self.assistant_message
    }

    pub fn bearer(&self) -> &str {
        &self.bearer
    }
}

impl fmt::Debug for AgentTitleGenerationRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentTitleGenerationRequest")
            .field("model", &self.model)
            .field(
                "owner_message",
                &format_args!("<redacted {} bytes>", self.owner_message.len()),
            )
            .field(
                "assistant_message",
                &format_args!("<redacted {} bytes>", self.assistant_message.len()),
            )
            .field(
                "bearer",
                &format_args!("<redacted {} bytes>", self.bearer.len()),
            )
            .finish()
    }
}

/// Untrusted text returned by a title generator, redacted until sanitization.
#[derive(Clone, PartialEq, Eq)]
pub struct AgentTitleGenerationResponse(String);

impl AgentTitleGenerationResponse {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for AgentTitleGenerationResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("AgentTitleGenerationResponse(<redacted>)")
    }
}

/// Content-free runtime failure suitable for logs and retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("agent title generation failed")]
pub struct AgentTitleGenerationError;

impl AgentTitleGenerationError {
    pub const fn unavailable() -> Self {
        Self
    }
}

/// Deployment model call made only while the caller owns generation authority.
#[async_trait]
pub trait AgentTitleGenerator: Send + Sync {
    async fn generate(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError>;
}

pub(crate) fn truncate_utf8_bytes(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
