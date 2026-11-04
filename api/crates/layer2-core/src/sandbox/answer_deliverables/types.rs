use super::DeliverableError;
use crate::AgentId;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

const PREFIX: &str = "<!--cma-deliverable:v1:";
const SUFFIX: &str = "-->";
const MAX_REFERENCES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DeliverableResource {
    AgentArtifact { id: String },
    Document { id: String, revision: u64 },
    LiveFile { path: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliverableReference {
    pub agent_id: AgentId,
    pub resource: DeliverableResource,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

impl DeliverableReference {
    pub fn document(
        agent_id: AgentId,
        id: String,
        revision: &crate::document_artifacts::DocumentRevision,
    ) -> Self {
        Self {
            agent_id,
            resource: DeliverableResource::Document {
                id,
                revision: revision.revision,
            },
            name: format!("{}.md", file_name(&revision.title)),
            media_type: "text/markdown".into(),
            size_bytes: revision.size_bytes,
            sha256: revision.sha256.clone(),
        }
    }

    fn validate(&self) -> Result<(), DeliverableError> {
        if self.agent_id.as_str().is_empty()
            || self.agent_id.as_str().len() > 128
            || self.name.is_empty()
            || self.name.len() > 160
            || self.name.trim() != self.name
            || matches!(self.name.as_str(), "." | "..")
            || self
                .name
                .chars()
                .any(|character| character.is_control() || matches!(character, '/' | '\\'))
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DeliverableError::Invalid);
        }
        let valid = match &self.resource {
            DeliverableResource::LiveFile { path } => {
                layer3_adapter::valid_agent_file_path(path)
                    && !path.is_empty()
                    && self.name == file_name(path.rsplit('/').next().unwrap_or("deliverable"))
                    && self.media_type == "application/octet-stream"
                    && self.size_bytes <= layer3_adapter::MAX_AGENT_FILE_BYTES
            }
            DeliverableResource::AgentArtifact { id } => {
                super::super::agent_artifacts::valid_artifact_id(id)
                    && (1..=super::super::agent_artifacts::MAX_ARTIFACT_BYTES as u64)
                        .contains(&self.size_bytes)
                    && matches!(
                        self.media_type.as_str(),
                        "text/plain"
                            | "text/markdown"
                            | "application/json"
                            | "application/pdf"
                            | "application/octet-stream"
                            | "image/png"
                            | "image/jpeg"
                            | "image/webp"
                    )
            }
            DeliverableResource::Document { id, revision } => {
                crate::document_artifacts::valid_id(id, "doc_")
                    && *revision > 0
                    && self.media_type == "text/markdown"
                    && (1..=crate::document_artifacts::MAX_DOCUMENT_BYTES as u64)
                        .contains(&self.size_bytes)
            }
        };
        if !valid {
            return Err(DeliverableError::Invalid);
        }
        Ok(())
    }

    pub fn marker(&self) -> Result<String, DeliverableError> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| DeliverableError::Invalid)?;
        let encoded = URL_SAFE_NO_PAD.encode(bytes);
        if encoded.len() > 4096 {
            return Err(DeliverableError::Invalid);
        }
        Ok(format!("{PREFIX}{encoded}{SUFFIX}"))
    }
    pub fn id(&self) -> Result<String, DeliverableError> {
        let bytes = serde_json::to_vec(&(
            &self.agent_id,
            &self.resource,
            &self.sha256,
            self.size_bytes,
        ))
        .map_err(|_| DeliverableError::Invalid)?;
        Ok(format!("delivery_{}", digest(&bytes)))
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DeliverableDescription {
    pub reference: DeliverableReference,
    pub marker: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliverableStatus {
    Ready,
    Unchecked,
    Changed,
    VersionChanged,
    Missing,
    Denied,
    NotReady,
    TooLarge,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnswerDeliverable {
    pub id: String,
    pub reference: DeliverableReference,
    pub status: DeliverableStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnswerDeliverables {
    pub agent_id: AgentId,
    pub answer_id: String,
    pub turn_sequence: u64,
    pub items: Vec<AnswerDeliverable>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnswerDeliverableReading {
    pub agent_id: AgentId,
    pub answer_id: String,
    pub turn_sequence: u64,
    pub item: AnswerDeliverable,
}

pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn file_name(value: &str) -> String {
    let mut result = String::new();
    for character in value.chars() {
        let character = if character.is_control() || matches!(character, '/' | '\\') {
            '_'
        } else {
            character
        };
        if result.len() + character.len_utf8() > 150 {
            break;
        }
        result.push(character);
    }
    if result.trim().is_empty() || matches!(result.as_str(), "." | "..") {
        "deliverable".into()
    } else {
        result.trim().to_owned()
    }
}

pub(super) fn references(
    content: &str,
    agent: &AgentId,
) -> Result<Vec<DeliverableReference>, DeliverableError> {
    let mut references = Vec::new();
    let mut identities = std::collections::BTreeSet::new();
    let mut fence: Option<(char, usize)> = None;
    for line in content.lines() {
        let trimmed = line.trim_start();
        let character = trimmed.chars().next().unwrap_or(' ');
        let count = trimmed
            .chars()
            .take_while(|value| *value == character)
            .count();
        if matches!(character, '`' | '~') && count >= 3 {
            if let Some((opened, width)) = fence {
                if character == opened && count >= width && trimmed[count..].trim().is_empty() {
                    fence = None;
                }
            } else {
                fence = Some((character, count));
            }
            continue;
        }
        if fence.is_some() {
            continue;
        }
        let Some(encoded) = line
            .strip_prefix(PREFIX)
            .and_then(|value| value.strip_suffix(SUFFIX))
        else {
            continue;
        };
        if encoded.len() > 4096 {
            return Err(DeliverableError::Invalid);
        }
        let bytes = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| DeliverableError::Invalid)?;
        let reference: DeliverableReference =
            serde_json::from_slice(&bytes).map_err(|_| DeliverableError::Invalid)?;
        reference.validate()?;
        if reference.agent_id != *agent {
            return Err(DeliverableError::Invalid);
        }
        if identities.insert(reference.id()?) {
            references.push(reference);
        }
        if references.len() > MAX_REFERENCES {
            return Err(DeliverableError::Invalid);
        }
    }
    Ok(references)
}
