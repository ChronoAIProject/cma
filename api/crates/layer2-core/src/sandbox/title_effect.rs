//! Retained custody of one automatic-title obligation per Agent lifetime.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use super::{
    AgentTitle, AgentTitleClaim, AgentTitleGenerationError, AgentTitleGenerationResponse,
    AgentTitleStoreError, AgentTitleTranscript,
};

/// Preparation must not send or spawn a send. Dispatch consumes the original
/// capability; durable rediscovery cannot reconstruct it. Implementations retain
/// their original frozen request, capacity permit and monotonic deadline.
#[async_trait]
pub trait PreparedAgentTitle: Send {
    fn request_digest(&self) -> &str;
    fn strategy(&self) -> &str;
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError>;
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTitleEffectIntent {
    pub agent_id: crate::AgentId,
    pub nonce: String,
    pub holder: String,
    pub claim_token: String,
    pub acquired_ms: u64,
    pub expires_ms: u64,
    pub turn_sequence: u64,
    pub transcript_digest: String,
    pub request_digest: String,
    pub strategy: String,
}

impl AgentTitleEffectIntent {
    pub fn new(
        claim: &AgentTitleClaim,
        transcript: &AgentTitleTranscript,
        prepared: &dyn PreparedAgentTitle,
    ) -> Result<Self, AgentTitleStoreError> {
        let intent = Self {
            agent_id: claim.agent_id().clone(),
            nonce: uuid::Uuid::new_v4().to_string(),
            holder: claim.holder().into(),
            claim_token: claim.token().into(),
            acquired_ms: millis(claim.acquired_at())?,
            expires_ms: millis(claim.expires_at())?,
            turn_sequence: transcript.turn_sequence(),
            transcript_digest: crate::profiles::content_digest(&(
                transcript.turn_sequence(),
                transcript.owner_message(),
                transcript.assistant_message(),
            ))
            .map_err(|_| AgentTitleStoreError::Corrupt)?,
            request_digest: prepared.request_digest().into(),
            strategy: prepared.strategy().into(),
        };
        intent.validate()?;
        Ok(intent)
    }

    pub fn validate(&self) -> Result<(), AgentTitleStoreError> {
        if self.agent_id.as_str().is_empty()
            || self.agent_id.as_str().len() > 256
            || uuid::Uuid::parse_str(&self.nonce).is_err()
            || self.holder.trim().is_empty()
            || self.holder.len() > 256
            || self.claim_token.is_empty()
            || self.claim_token.len() > 256
            || self.strategy.is_empty()
            || self.strategy.len() > 128
            || self.turn_sequence == 0
            || self.turn_sequence > i64::MAX as u64
            || self.expires_ms > i64::MAX as u64
            || self.expires_ms.checked_sub(self.acquired_ms)
                != Some(super::AGENT_TITLE_CLAIM_SECONDS * 1000)
            || !digest(&self.transcript_digest)
            || !digest(&self.request_digest)
        {
            return Err(AgentTitleStoreError::Corrupt);
        }
        Ok(())
    }

    pub fn matches_claim(&self, claim: &AgentTitleClaim) -> bool {
        self.agent_id == *claim.agent_id()
            && self.holder == claim.holder()
            && self.claim_token == claim.token()
            && millis(claim.acquired_at()).ok() == Some(self.acquired_ms)
            && millis(claim.expires_at()).ok() == Some(self.expires_ms)
            && claim.mode() == super::AgentTitleClaimMode::Generate
    }
}

/// Contains only a bounded sanitized title; Debug deliberately omits it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTitleEffect {
    pub version: u32,
    pub intent: AgentTitleEffectIntent,
    pub result: Option<AgentTitle>,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub enum AgentTitleEffectState {
    #[default]
    Legacy,
    Ready,
    /// An old writer attempted generation without retained provider evidence.
    LegacyUnknown,
    Retained(Box<AgentTitleEffect>),
}

impl AgentTitleEffectState {
    pub fn unknown(&self) -> bool {
        matches!(self, Self::LegacyUnknown)
            || matches!(self, Self::Retained(effect) if effect.result.is_none())
    }
    pub fn accepted(&self) -> Option<&AgentTitle> {
        match self {
            Self::Retained(effect) => effect.result.as_ref(),
            _ => None,
        }
    }
    pub fn permits_title(&self, title: &AgentTitle, source: super::AgentTitleSource) -> bool {
        match self {
            Self::Legacy => true,
            Self::Ready => source == super::AgentTitleSource::Fallback,
            // An unresolved provider effect forbids redispatch, not the
            // prompt-derived fallback: it sends nothing and still leaves the
            // effect unresolved for purge finality (#2266).
            Self::LegacyUnknown => source == super::AgentTitleSource::Fallback,
            Self::Retained(effect) => match effect.result.as_ref() {
                Some(result) => source == super::AgentTitleSource::Generated && result == title,
                None => source == super::AgentTitleSource::Fallback,
            },
        }
    }
}

fn millis(time: SystemTime) -> Result<u64, AgentTitleStoreError> {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .ok()
        .and_then(|value| u64::try_from(value.as_millis()).ok())
        .ok_or(AgentTitleStoreError::InvalidClaim)
}
fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}
