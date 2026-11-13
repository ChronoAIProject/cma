use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{TriggerContent, TriggerError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TriggerRevision {
    pub revision: u32,
    pub content: TriggerContent,
    pub digest: String,
    pub published_at: String,
    /// A retained identity for an erased publisher, never executable content.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub publisher_deleted: bool,
}

impl TriggerRevision {
    pub fn new(
        revision: u32,
        content: TriggerContent,
        published_at: String,
    ) -> Result<Self, TriggerError> {
        content.validate()?;
        if revision == 0 {
            return Err(TriggerError::new("invalid_trigger_revision", "revision"));
        }
        if published_at.trim().is_empty() {
            return Err(TriggerError::new("invalid_timestamp", "published_at"));
        }
        Ok(Self {
            revision,
            digest: trigger_content_digest(&content)?,
            content,
            published_at,
            publisher_deleted: false,
        })
    }

    pub fn validate(&self) -> Result<(), TriggerError> {
        if self.revision == 0 {
            return Err(TriggerError::new("invalid_trigger_revision", "revision"));
        }
        if self.published_at.trim().is_empty() {
            return Err(TriggerError::new("invalid_timestamp", "published_at"));
        }
        if self.publisher_deleted {
            if self.content != erased_content()
                || self.digest.len() != 64
                || !self
                    .digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(TriggerError::new("invalid_publisher_tombstone", "content"));
            }
            return Ok(());
        }
        self.content.validate()?;
        if self.digest != trigger_content_digest(&self.content)? {
            return Err(TriggerError::new("trigger_digest_mismatch", "digest"));
        }
        Ok(())
    }

    /// Preserve the admitted identity while removing all publisher-authored data.
    pub fn erase_publisher(&mut self) {
        self.content = erased_content();
        self.publisher_deleted = true;
    }
}

fn erased_content() -> TriggerContent {
    use super::{
        AgentReusePolicy, ButtonLabel, Presentation, ReusePolicy, SandboxReusePolicy, Theme,
        TriggerConsent, TriggerTarget, WorkspaceReusePolicy,
    };
    use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};
    TriggerContent {
        presentation: Presentation {
            title: String::new(),
            description: String::new(),
            button_label: ButtonLabel::OpenInCma,
            theme: Theme::Auto,
            signup_hint: None,
        },
        target: TriggerTarget {
            sandbox_profile: SandboxProfileRef::new(crate::SandboxProfileId(String::new()), 0),
            workspace_profile: WorkspaceProfileRef::new(
                crate::WorkspaceProfileId(String::new()),
                0,
            ),
            agent_profile: AgentProfileRef::new(crate::AgentProfileId(String::new()), 0),
            inline: None,
        },
        reuse: ReusePolicy {
            sandbox: SandboxReusePolicy::AlwaysNew,
            workspace: WorkspaceReusePolicy::AlwaysNew,
            agent: AgentReusePolicy::AlwaysNew,
        },
        first_message: String::new(),
        quick_messages: Vec::new(),
        consent: TriggerConsent {
            always_review: true,
        },
    }
}

/// SHA-256 of canonical JSON with recursively sorted object keys and no whitespace.
pub fn trigger_content_digest(content: &TriggerContent) -> Result<String, TriggerError> {
    let value = serde_json::to_value(content)
        .map_err(|_| TriggerError::new("content_not_serializable", "content"))?;
    let bytes = serde_json::to_vec(&canonicalize(value))
        .map_err(|_| TriggerError::new("content_not_serializable", "content"))?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn canonicalize(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(canonicalize).collect()),
        Value::Object(values) => {
            let mut entries: Vec<_> = values.into_iter().collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, canonicalize(value)))
                    .collect::<Map<_, _>>(),
            )
        }
        scalar => scalar,
    }
}
