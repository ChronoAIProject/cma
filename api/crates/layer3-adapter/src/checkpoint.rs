use serde::{Deserialize, Serialize};

/// Capture the complete sandbox home under one writer fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointRequest {
    pub epoch: u64,
    #[serde(default)]
    pub exclusions: Vec<String>,
}

/// State of every agent writer when checkpoint capture returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriterState {
    Resumed,
    Held,
}

/// Sealed object-store checkpoint receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReceipt {
    pub epoch: u64,
    pub object_reference: String,
    pub sha256: String,
    pub writers: WriterState,
}

/// Capture request whose writer fence remains held for provider teardown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointHoldRequest {
    pub checkpoint: CheckpointRequest,
    pub writer_resume_token: String,
}

impl CheckpointHoldRequest {
    pub fn has_valid_writer_resume_token(&self) -> bool {
        valid_writer_resume_token(&self.writer_resume_token)
    }
}

/// Sealed checkpoint receipt proving that runtime writers remain held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointHoldReceipt {
    pub checkpoint: CheckpointReceipt,
    pub writer_resume_token: String,
}

/// Idempotent request to release one retained checkpoint writer fence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointWriterResumeRequest {
    pub writer_resume_token: String,
}

impl CheckpointWriterResumeRequest {
    pub fn has_valid_writer_resume_token(&self) -> bool {
        valid_writer_resume_token(&self.writer_resume_token)
    }
}

/// Acknowledgement that a retained writer fence is no longer held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointWriterResumeReceipt {
    pub writer_resume_token: String,
    pub writers_resumed: bool,
}

/// Resume tokens are caller-issued, canonical SHA-256-shaped identities.
pub fn valid_writer_resume_token(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Compare two already validated fixed-width resume tokens without early exit.
pub fn writer_resume_tokens_match(left: &str, right: &str) -> bool {
    let mut difference = left.len() ^ right.len();
    for (left, right) in left.bytes().zip(right.bytes()) {
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

/// Restore one previously sealed whole-home checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreRequest {
    pub object_reference: String,
    pub sha256: String,
}

/// One direct resource reading from the sandbox supervisor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourceReading {
    pub uptime_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage_usec: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_millis_limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes_peak: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stall_permille: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ceiling_hits: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocation_failures: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kills: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_resume_tokens_are_fixed_width_lowercase_hex() {
        let token = "a".repeat(64);
        assert!(valid_writer_resume_token(&token));
        assert!(writer_resume_tokens_match(&token, &token));
        assert!(!valid_writer_resume_token(&"A".repeat(64)));
        assert!(!valid_writer_resume_token(&"a".repeat(63)));
        assert!(!writer_resume_tokens_match(&token, &"b".repeat(64)));
    }
}
