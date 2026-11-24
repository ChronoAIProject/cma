//! Runtime evidence for settling an already accepted prompt.
use serde::{Deserialize, Serialize};

use crate::{AgentSlot, PromptIdentity, RuntimeHandle, RuntimeOutputSource};

pub const SEAL_VERSION: u32 = 1;
pub const MAX_SEAL_BYTES: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleTurnTarget {
    pub sandbox_id: String,
    pub slot: AgentSlot,
    pub runtime: RuntimeHandle,
    pub runtime_boot: String,
    pub runtime_generation: u64,
    pub conversation_id: String,
    pub conversation_generation: u64,
    pub command_id: String,
    pub operation_sequence: u64,
    pub turn_sequence: u64,
    pub writer_epoch: u64,
    pub stream_id: Option<String>,
}

impl StaleTurnTarget {
    pub fn prompt(&self) -> PromptIdentity {
        PromptIdentity::new(
            &self.conversation_id,
            self.conversation_generation,
            self.turn_sequence,
        )
    }

    pub fn is_valid(&self) -> bool {
        identifier(&self.sandbox_id, 256)
            && identifier(&self.slot.0, 256)
            && identifier(self.runtime.as_str(), 512)
            && hex(&self.runtime_boot, 32)
            && self.runtime_generation != 0
            && identifier(&self.conversation_id, 1024)
            && self.conversation_generation != 0
            && identifier(&self.command_id, 256)
            && self.operation_sequence != 0
            && self.turn_sequence != 0
            && self.writer_epoch != 0
            && self.stream_id.as_deref().is_none_or(valid_stream)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "position", rename_all = "snake_case", deny_unknown_fields)]
pub enum SealedOutput {
    Empty { stream_id: String },
    Through { source: RuntimeOutputSource },
}

/// An authenticated provider operation has stopped this exact accepted prompt,
/// with no canonical result and with all of its producers ordered before this
/// final output position. The provider must keep this receipt across handoff.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StaleTurnSeal {
    pub version: u32,
    pub target: StaleTurnTarget,
    pub prompt_id: String,
    pub stop_receipt_id: String,
    pub output: SealedOutput,
}

impl StaleTurnSeal {
    pub fn matches(&self, expected: &StaleTurnTarget) -> bool {
        if &self.target != expected
            || self.version != SEAL_VERSION
            || !expected.is_valid()
            || self.prompt_id != expected.prompt().as_str()
            || !identifier(&self.stop_receipt_id, 256)
        {
            return false;
        }
        match &self.output {
            SealedOutput::Empty { stream_id } => {
                valid_stream(stream_id)
                    && expected
                        .stream_id
                        .as_ref()
                        .is_none_or(|stream| stream == stream_id)
            }
            SealedOutput::Through { source } => {
                valid_stream(&source.stream_id)
                    && expected
                        .stream_id
                        .as_ref()
                        .is_none_or(|stream| stream == &source.stream_id)
                    && source.native_conversation_id == expected.conversation_id
                    && (1..u64::MAX).contains(&source.cursor)
                    && hex(&source.payload_sha256, 64)
            }
        }
    }

    pub fn matches_durable_output(&self, durable: Option<&RuntimeOutputSource>) -> bool {
        match (&self.output, durable) {
            (SealedOutput::Empty { .. }, None) => true,
            (SealedOutput::Through { source }, Some(durable)) => source == durable,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StaleTurnHold {
    #[error("the producer seal is unsupported")]
    Unsupported,
    #[error("the producer has not proved a final output boundary")]
    Unconfirmed,
    #[error("a canonical result is available through the existing recorder")]
    CanonicalResult,
}

pub struct StaleTurnEvidence {
    pub seal: StaleTurnSeal,
    pub bytes_read: usize,
}

/// Only an adapter that can authenticate the exact producer's durable seal may
/// implement this capability. Observation cannot dispatch or cancel input, and
/// a cancelled read cannot leave a detached probe running.
#[async_trait::async_trait]
pub trait StaleTurnProducer: Send + Sync {
    /// Stop reading producer evidence at the supplied cumulative byte bound.
    async fn observe_stale_turn_seal(
        &self,
        target: &StaleTurnTarget,
        max_evidence_bytes: usize,
    ) -> Result<StaleTurnEvidence, StaleTurnHold>;

    /// Submit one exact stop after the caller has revalidated ownership and eligibility.
    /// Acknowledgement only confirms submission. Observe the ordered result separately.
    async fn request_stale_turn_stop(
        &self,
        _target: &StaleTurnTarget,
    ) -> Result<(), StaleTurnHold> {
        Err(StaleTurnHold::Unsupported)
    }
}

fn valid_stream(value: &str) -> bool {
    value.len() == 36 && uuid::Uuid::parse_str(value).is_ok()
}

fn identifier(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_turn_seal_binds_prompt_writer_and_durable_payload() {
        let stream_id = "b7111111-1111-4111-8111-111111111111".to_owned();
        let target = StaleTurnTarget {
            sandbox_id: "sandbox".into(),
            slot: AgentSlot("agent".into()),
            runtime: RuntimeHandle("runtime".into()),
            runtime_boot: "b".repeat(32),
            runtime_generation: 12,
            conversation_id: "conversation".into(),
            conversation_generation: 5,
            command_id: "command".into(),
            operation_sequence: 3,
            turn_sequence: 6,
            writer_epoch: 8,
            stream_id: Some(stream_id.clone()),
        };
        let source = RuntimeOutputSource {
            stream_id,
            cursor: 2,
            native_conversation_id: target.conversation_id.clone(),
            payload_sha256: "a".repeat(64),
        };
        let mut seal = StaleTurnSeal {
            version: SEAL_VERSION,
            prompt_id: target.prompt().as_str().to_owned(),
            stop_receipt_id: "receipt".into(),
            target: target.clone(),
            output: SealedOutput::Through {
                source: source.clone(),
            },
        };
        assert!(seal.matches(&target));
        assert!(seal.matches_durable_output(Some(&source)));
        let mut later = source.clone();
        later.cursor += 1;
        assert!(!seal.matches_durable_output(Some(&later)));
        later = source.clone();
        later.payload_sha256 = "b".repeat(64);
        assert!(!seal.matches_durable_output(Some(&later)));
        assert!(!seal.matches_durable_output(None));

        let mut successor = target.clone();
        successor.writer_epoch += 1;
        assert!(!seal.matches(&successor));
        let mut other_prompt = target.clone();
        other_prompt.turn_sequence += 1;
        seal.prompt_id = other_prompt.prompt().as_str().to_owned();
        assert!(!seal.matches(&target));
        seal.prompt_id = target.prompt().as_str().to_owned();
        seal.output = SealedOutput::Empty {
            stream_id: source.stream_id.clone(),
        };
        assert!(seal.matches(&target));
        assert!(seal.matches_durable_output(None));
        assert!(!seal.matches_durable_output(Some(&source)));
    }
}
