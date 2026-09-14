use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

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

impl RestoreRequest {
    /// Fingerprint the exact restore locator without making it durable itself.
    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.restore.request.v1\0");
        hash_string(&mut digest, &self.object_reference);
        hash_string(&mut digest, &self.sha256);
        hex(&digest.finalize())
    }

    pub fn has_valid_reference(&self) -> bool {
        lower_sha256(&self.sha256)
            && self
                .object_reference
                .strip_prefix("home-")
                .and_then(|value| value.strip_suffix(".cma"))
                == Some(self.sha256.as_str())
    }
}

/// Admit a replacement runtime using software carried by one exact checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBootstrapRequest {
    pub operation_id: String,
    pub sandbox_id: String,
    pub checkpoint: RestoreRequest,
    pub request_sha256: String,
}

impl RestoreBootstrapRequest {
    pub fn new(
        operation_id: impl Into<String>,
        sandbox_id: impl Into<String>,
        checkpoint: RestoreRequest,
    ) -> Self {
        let mut request = Self {
            operation_id: operation_id.into(),
            sandbox_id: sandbox_id.into(),
            checkpoint,
            request_sha256: String::new(),
        };
        request.request_sha256 = request.computed_sha256();
        request
    }

    pub fn computed_sha256(&self) -> String {
        let mut digest = Sha256::new();
        digest.update(b"chrono.restore.bootstrap.request.v1\0");
        hash_string(&mut digest, &self.operation_id);
        hash_string(&mut digest, &self.sandbox_id);
        hash_string(&mut digest, &self.checkpoint.computed_sha256());
        hex(&digest.finalize())
    }

    pub fn has_valid_sha256(&self) -> bool {
        self.request_sha256 == self.computed_sha256()
    }
}

/// Durable, non-secret correlation proving inherited installation admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreBootstrapReceipt {
    pub operation_id: String,
    pub request_sha256: String,
    pub restore_request_sha256: String,
    pub installation_inherited: bool,
}

fn hash_string(digest: &mut Sha256, value: &str) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value.as_bytes());
}

fn lower_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
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

    #[test]
    fn restore_bootstrap_digest_covers_operation_sandbox_and_exact_checkpoint() {
        let sha256 = "a".repeat(64);
        let checkpoint = RestoreRequest {
            object_reference: format!("home-{sha256}.cma"),
            sha256,
        };
        assert!(checkpoint.has_valid_reference());
        let original = RestoreBootstrapRequest::new("restore-1", "sandbox-1", checkpoint);
        assert!(original.has_valid_sha256());

        let mut variants = [original.clone(), original.clone(), original.clone()];
        variants[0].operation_id = "restore-2".into();
        variants[1].sandbox_id = "sandbox-2".into();
        variants[2].checkpoint.sha256 = "b".repeat(64);
        assert!(variants
            .iter()
            .all(|changed| changed.computed_sha256() != original.request_sha256));
    }
}
