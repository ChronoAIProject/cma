use serde::{Deserialize, Serialize};

use crate::bridge::BridgeSecretRef;
use crate::resource_access::ResourceActor;

use super::{ChannelReplyToolIntent, OutboxError, OutboxSource};

/// References only. IDs, historical custody and serialized records confer no
/// authority; the delivery adapter resolves and verifies actual current material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliveryCredentialRef {
    ReplyToken {
        inbox_id: String,
        expires_at_ms: i64,
    },
    RetainedKey {
        reference: BridgeSecretRef,
        revision: u64,
        key_state_version: i64,
    },
}
impl DeliveryCredentialRef {
    pub fn is_reply_token(&self) -> bool {
        matches!(self, Self::ReplyToken { .. })
    }
    pub(super) fn require_source(
        &self,
        source: &OutboxSource,
        now_ms: i64,
    ) -> Result<(), OutboxError> {
        match self {
            Self::ReplyToken {
                inbox_id,
                expires_at_ms,
            } if inbox_id == source.inbox_id()
                && Some(*expires_at_ms) == source.reply_expires_at_ms =>
            {
                if now_ms >= *expires_at_ms {
                    return Err(OutboxError::Expired);
                }
            }
            Self::RetainedKey {
                reference,
                revision,
                key_state_version,
            } if reference == &source.config.connector.custody
                && *revision > 0
                && *revision <= i64::MAX as u64
                && *key_state_version > 0 => {}
            _ => return Err(OutboxError::SourceMismatch),
        }
        if now_ms < 0
            || source
                .route_expires_at_ms
                .is_some_and(|deadline| now_ms >= deadline)
        {
            return Err(OutboxError::Expired);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RelayOperation {
    InitialReply,
    Edit {
        original_outbox_id: String,
        platform_message_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryFailure {
    CredentialExpired,
    CredentialRevoked,
    BotInactive,
    RouteUnavailable,
    CapabilityUnsupported,
    RateLimited,
    RequestRejected,
    ConnectUnavailable,
    ProviderFailure,
    ResponseLost,
    WorkerLost,
    ReceiptInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryReceipt {
    pub nyxid_message_id: String,
    pub platform_message_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliveryState {
    Pending,
    Claimed {
        attempt: String,
        holder: String,
        expires_at_ms: i64,
    },
    Sending {
        attempt: String,
        expires_at_ms: i64,
    },
    RetryWait {
        retry_at_ms: i64,
        reason: DeliveryFailure,
    },
    Sent {
        receipt: DeliveryReceipt,
    },
    Rejected {
        reason: DeliveryFailure,
    },
    Unknown {
        attempt: String,
        reason: DeliveryFailure,
    },
    Abandoned {
        actor: ResourceActor,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryAttempt {
    pub id: String,
    pub number: u32,
    pub started_at_ms: i64,
    pub credential: DeliveryCredentialRef,
    pub state: DeliveryState,
}

/// Protected outbound content and routing. Broad telemetry must use only an
/// explicit status projection; this record intentionally has no Debug.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutboxItem {
    pub id: String,
    pub source: OutboxSource,
    pub tool_invocation_id: String,
    pub turn_sequence: u64,
    pub sequence: u64,
    pub chunk_index: u32,
    pub chunk_count: u32,
    pub last_chunk: bool,
    pub predecessor: Option<String>,
    pub content_sha256: String,
    pub(super) text: String,
    pub credential: DeliveryCredentialRef,
    pub operation: RelayOperation,
    pub revision: u64,
    pub state: DeliveryState,
    pub attempts: Vec<DeliveryAttempt>,
    pub duplicate_risk: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl OutboxItem {
    /// Called by the enqueue transaction after reserving one response-order
    /// range. Every explicit tool call remains independent; no coalescing or
    /// terminal Agent-response prerequisite exists.
    pub fn from_tool_intent(
        intent: &ChannelReplyToolIntent,
        credential: DeliveryCredentialRef,
        sequence_start: u64,
        predecessor: Option<String>,
        now_ms: i64,
    ) -> Result<Vec<Self>, OutboxError> {
        intent.scope.require_current()?;
        let source = intent.scope.source();
        source.validate()?;
        credential.require_source(source, now_ms)?;
        let chunks = super::text::chunks(&source.config.connector.platform, &intent.text)?;
        if chunks.is_empty()
            || chunks.len() > 1024
            || sequence_start == 0
            || sequence_start
                .checked_add(chunks.len() as u64)
                .is_none_or(|n| n > i64::MAX as u64)
        {
            return Err(OutboxError::Bounds);
        }
        if credential.is_reply_token() && chunks.len() > 1 {
            return Err(OutboxError::RetainedKeyRequired);
        }
        let count = chunks.len() as u32;
        let mut previous = predecessor;
        let mut result = Vec::new();
        for (index, text) in chunks.into_iter().enumerate() {
            let mut item = Self {
                id: String::new(),
                source: source.clone(),
                tool_invocation_id: intent.id().into(),
                turn_sequence: intent.scope.turn_sequence(),
                sequence: sequence_start + index as u64,
                chunk_index: index as u32,
                chunk_count: count,
                last_chunk: index as u32 + 1 == count,
                predecessor: previous,
                content_sha256: super::hash(&text)?,
                text,
                credential: credential.clone(),
                operation: RelayOperation::InitialReply,
                revision: 1,
                state: DeliveryState::Pending,
                attempts: Vec::new(),
                duplicate_risk: false,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            };
            item.id = item.identity()?;
            item.validate()?;
            previous = Some(item.id.clone());
            result.push(item);
        }
        Ok(result)
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    fn identity(&self) -> Result<String, OutboxError> {
        Ok(format!(
            "bro_{}",
            super::hash(&(
                "cma.bridge.outbox.v1",
                self.source.fingerprint()?,
                &self.tool_invocation_id,
                self.turn_sequence,
                self.chunk_index,
                &self.operation
            ))?
        ))
    }
    pub fn require_same_output(&self, other: &Self) -> Result<(), OutboxError> {
        self.validate()?;
        other.validate()?;
        if self.id != other.id
            || self.source != other.source
            || self.text != other.text
            || self.content_sha256 != other.content_sha256
            || self.chunk_count != other.chunk_count
            || self.predecessor != other.predecessor
            || self.sequence != other.sequence
        {
            return Err(OutboxError::SourceMismatch);
        }
        Ok(())
    }
    pub fn validate(&self) -> Result<(), OutboxError> {
        self.source.validate()?;
        self.credential.require_source(&self.source, 0)?;
        if self.id != self.identity()?
            || self.content_sha256 != super::hash(&self.text)?
            || self.text.is_empty()
            || self.text.len() > 64_000
            || !super::id(&self.tool_invocation_id)
            || self.turn_sequence == 0
            || self.sequence == 0
            || self.chunk_count == 0
            || self.chunk_count > 1024
            || self.chunk_index >= self.chunk_count
            || self.last_chunk != (self.chunk_index + 1 == self.chunk_count)
            || (self.sequence == 1) != self.predecessor.is_none()
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.created_at_ms < 0
            || self.updated_at_ms < self.created_at_ms
            || self.attempts.len() > 16
        {
            return Err(OutboxError::Corrupt);
        }
        for (index, attempt) in self.attempts.iter().enumerate() {
            attempt.credential.require_source(&self.source, 0)?;
            if attempt.number != index as u32 + 1
                || attempt.id != self.attempt_id(attempt.number)?
                || attempt.started_at_ms < self.created_at_ms
                || attempt.started_at_ms > self.updated_at_ms
            {
                return Err(OutboxError::Corrupt);
            }
        }
        match &self.state {
            DeliveryState::Claimed {
                holder,
                expires_at_ms,
                ..
            } if !super::id(holder) || *expires_at_ms <= self.updated_at_ms => {
                return Err(OutboxError::Corrupt)
            }
            DeliveryState::Sending { expires_at_ms, .. } if *expires_at_ms <= 0 => {
                return Err(OutboxError::Corrupt)
            }
            DeliveryState::RetryWait { retry_at_ms, .. } if *retry_at_ms <= self.updated_at_ms => {
                return Err(OutboxError::Corrupt)
            }
            DeliveryState::Sent { receipt }
                if !super::id(&receipt.nyxid_message_id)
                    || receipt
                        .platform_message_id
                        .as_ref()
                        .is_some_and(|s| !super::id(s)) =>
            {
                return Err(OutboxError::Corrupt)
            }
            _ => {}
        }
        if let RelayOperation::Edit {
            original_outbox_id,
            platform_message_id,
        } = &self.operation
        {
            if !super::id(original_outbox_id)
                || !super::id(platform_message_id)
                || self.chunk_count != 1
            {
                return Err(OutboxError::Corrupt);
            }
        }
        if let DeliveryState::Claimed { attempt, .. }
        | DeliveryState::Sending { attempt, .. }
        | DeliveryState::Unknown { attempt, .. } = &self.state
        {
            if self
                .attempts
                .last()
                .is_none_or(|last| &last.id != attempt || last.state != self.state)
            {
                return Err(OutboxError::Corrupt);
            }
        }
        Ok(())
    }
    pub(super) fn attempt_id(&self, number: u32) -> Result<String, OutboxError> {
        Ok(format!(
            "broa_{}",
            super::hash(&("cma.bridge.outbox-attempt.v1", &self.id, number))?
        ))
    }
    pub(super) fn transition(
        &mut self,
        state: DeliveryState,
        now_ms: i64,
    ) -> Result<(), OutboxError> {
        if now_ms < self.updated_at_ms || self.revision >= i64::MAX as u64 {
            return Err(OutboxError::Conflict);
        }
        self.state = state;
        if let Some(last) = self.attempts.last_mut() {
            last.state = self.state.clone();
        }
        self.updated_at_ms = now_ms;
        self.revision += 1;
        self.validate()
    }
}
