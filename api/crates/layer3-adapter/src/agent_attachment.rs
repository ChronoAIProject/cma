//! Owner-supplied files confined to the retained home, independent of providers.
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use crate::{AdapterError, AgentSlot, Result};

pub const MAX_AGENT_ATTACHMENT_BYTES: u64 = 20 * 1024 * 1024;
pub const AGENT_ATTACHMENT_DIRECTORY: &str = "/home/attachments";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAttachmentRequest {
    pub sandbox_id: String,
    pub runtime_generation: u64,
    pub boot_id: String,
    pub agent_id: AgentSlot,
    pub attachment_id: String,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl AgentAttachmentRequest {
    pub fn validate(&self) -> Result<()> {
        if !component(&self.sandbox_id, 256)
            || self.runtime_generation == 0
            || !hex(&self.boot_id, 32)
            || !component(self.agent_id.as_str(), 64)
            || !self
                .attachment_id
                .strip_prefix("att_")
                .is_some_and(|id| hex(id, 64))
            || !component(&self.name, 120)
            || self.media_type.is_empty()
            || self.media_type.len() > 127
            || !self
                .media_type
                .bytes()
                .all(|b| b.is_ascii_graphic() && b != b'"')
            || self.size_bytes > MAX_AGENT_ATTACHMENT_BYTES
            || !hex(&self.sha256, 64)
        {
            return Err(invalid("attachment metadata is invalid"));
        }
        Ok(())
    }

    pub fn path(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            AGENT_ATTACHMENT_DIRECTORY,
            self.agent_id.as_str(),
            self.attachment_id,
            self.name
        )
    }

    pub fn validate_content(&self, content: &[u8]) -> Result<()> {
        self.validate()?;
        if content.len() as u64 != self.size_bytes
            || format!("{:x}", Sha256::digest(content)) != self.sha256
        {
            return Err(invalid(
                "attachment content does not match its declared size and digest",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAttachmentReceipt {
    pub attachment_id: String,
    pub agent_id: AgentSlot,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl AgentAttachmentReceipt {
    pub fn for_request(request: &AgentAttachmentRequest) -> Self {
        Self {
            attachment_id: request.attachment_id.clone(),
            agent_id: request.agent_id.clone(),
            path: request.path(),
            size_bytes: request.size_bytes,
            sha256: request.sha256.clone(),
        }
    }

    pub fn validate_for(&self, request: &AgentAttachmentRequest) -> Result<()> {
        request.validate()?;
        if self != &Self::for_request(request) {
            return Err(invalid("runtime returned another attachment receipt"));
        }
        Ok(())
    }
}

/// Bounded binary content encoded only on the private runtime control channel.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentAttachmentUpload {
    pub request: AgentAttachmentRequest,
    pub content_base64: String,
}

impl AgentAttachmentUpload {
    pub fn new(request: AgentAttachmentRequest, content: &[u8]) -> Result<Self> {
        request.validate_content(content)?;
        Ok(Self {
            request,
            content_base64: base64::engine::general_purpose::STANDARD.encode(content),
        })
    }

    pub fn content(&self) -> Result<Vec<u8>> {
        self.request.validate()?;
        if self.content_base64.len() as u64 > MAX_AGENT_ATTACHMENT_BYTES.div_ceil(3) * 4 {
            return Err(invalid("attachment content exceeds its size limit"));
        }
        let content = base64::engine::general_purpose::STANDARD
            .decode(&self.content_base64)
            .map_err(|_| invalid("attachment content encoding is invalid"))?;
        self.request.validate_content(&content)?;
        Ok(content)
    }
}

fn component(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn invalid(detail: &str) -> AdapterError {
    AdapterError::Rejected(detail.into())
}

impl std::fmt::Debug for AgentAttachmentUpload {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentAttachmentUpload")
            .field("request", &self.request)
            .field("encoded_content_bytes", &self.content_base64.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(content: &[u8]) -> AgentAttachmentRequest {
        AgentAttachmentRequest {
            sandbox_id: "sbx_test".into(),
            runtime_generation: 1,
            boot_id: "a".repeat(32),
            agent_id: AgentSlot::new("agt_test"),
            attachment_id: format!("att_{}", "b".repeat(64)),
            name: "report.txt".into(),
            media_type: "text/plain".into(),
            size_bytes: content.len() as u64,
            sha256: format!("{:x}", Sha256::digest(content)),
        }
    }
    #[test]
    fn uploads_bound_content_and_correlate_the_complete_receipt() {
        for content in [b"".as_slice(), b"private attachment bytes"] {
            let request = request(content);
            let upload = AgentAttachmentUpload::new(request.clone(), content).unwrap();
            assert_eq!(upload.content().unwrap(), content);
            assert!(!format!("{upload:?}").contains("cHJpdmF0ZSBhdHRhY2htZW50IGJ5dGVz"));
            let receipt = AgentAttachmentReceipt::for_request(&request);
            receipt.validate_for(&request).unwrap();
            let mut wrong = receipt.clone();
            wrong.path.push_str("/escape");
            assert!(wrong.validate_for(&request).is_err());
            let mut wrong = upload.clone();
            wrong.request.size_bytes += 1;
            assert!(wrong.content().is_err());
        }
        for bad in [
            "../escape",
            ".secret",
            "/etc/passwd",
            "a/b",
            "a\\b",
            "two names",
            "nul\0",
        ] {
            let mut item = request(b"x");
            item.name = bad.into();
            assert!(item.validate().is_err(), "{bad}");
        }
        let mut item = request(b"x");
        item.size_bytes = MAX_AGENT_ATTACHMENT_BYTES + 1;
        assert!(item.validate().is_err());
    }
}
