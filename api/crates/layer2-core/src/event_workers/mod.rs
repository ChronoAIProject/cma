//! Owned, revisioned Event Worker Profiles composed from published CMA profiles.
mod memory;
pub mod projection;
mod service;
use crate::{
    bots::{BotError, BotProfiles},
    profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef},
    SubjectId,
};
pub use memory::InMemoryEventWorkerProfileStore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use service::EventWorkerProfileService;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventWorkerProfileRef {
    pub id: String,
    pub revision: u32,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventWorkerProfileInput {
    pub name: String,
    pub description: String,
    pub agent_profile: AgentProfileRef,
    pub workspace_profile: WorkspaceProfileRef,
    pub sandbox_profile: SandboxProfileRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventWorkerProfilePin {
    pub reference: EventWorkerProfileRef,
    pub snapshot: Value,
    pub digest: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventWorkerProfile {
    pub provider: String,
    pub id: String,
    pub owner: SubjectId,
    pub revision: u32,
    pub source: EventWorkerProfileInput,
    pub profiles: BotProfiles,
    pub snapshot: Value,
    pub digest: String,
    pub created_at: String,
    pub updated_at: String,
    pub retired_at: Option<String>,
    pub creation_fingerprint: String,
}
impl EventWorkerProfile {
    pub fn validate(&self) -> Result<(), BotError> {
        validate_id(&self.id)?;
        let mut expected = projection::snapshot(&projection::WorkerProjection {
            name: &self.source.name,
            owner: &self.owner,
            provider: &self.provider,
            profiles: &self.profiles,
        })?;
        expected["metadata"]["description"] = Value::String(self.source.description.clone());
        if expected != self.snapshot {
            return Err(BotError::Unavailable);
        }

        if self.owner.as_str().is_empty()
            || self.source.name.trim().is_empty()
            || self.source.name.len() > 80
            || self.source.name.chars().any(char::is_control)
            || self.source.description.len() > 2000
            || self.revision == 0
            || self.profiles.event_worker_profile.is_some()
            || self.snapshot["metadata"]["owner"]["subject"].as_str() != Some(self.owner.as_str())
            || self.digest
                != format!(
                    "sha256:{}",
                    crate::profiles::content_digest(&self.snapshot)
                        .map_err(|_| BotError::Unavailable)?
                )
            || self.source.agent_profile.id != self.profiles.agent.profile_id
            || self.source.agent_profile.revision != self.profiles.agent.revision
            || self.source.workspace_profile.id != self.profiles.workspace.profile_id
            || self.source.workspace_profile.revision != self.profiles.workspace.revision
            || self.source.sandbox_profile.id != self.profiles.sandbox.profile_id
            || self.source.sandbox_profile.revision != self.profiles.sandbox.revision
        {
            return Err(BotError::Unavailable);
        }
        Ok(())
    }
    pub fn pin(&self) -> EventWorkerProfilePin {
        EventWorkerProfilePin {
            reference: EventWorkerProfileRef {
                id: self.id.clone(),
                revision: self.revision,
            },
            snapshot: self.snapshot.clone(),
            digest: self.digest.clone(),
        }
    }
}
pub fn validate_id(id: &str) -> Result<(), BotError> {
    if !id.strip_prefix("ewp_").is_some_and(|s| {
        s.len() == 32
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(BotError::NotFound);
    }
    Ok(())
}
#[async_trait::async_trait]
pub trait EventWorkerProfileStore: Send + Sync {
    async fn get(
        &self,
        id: &str,
        revision: Option<u32>,
    ) -> Result<Option<EventWorkerProfile>, BotError>;
    async fn list(
        &self,
        owner: &SubjectId,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<EventWorkerProfile>, BotError>;
    /// Atomically write the latest record and its immutable revision. None means insert.
    async fn save(&self, expected: Option<u32>, value: &EventWorkerProfile)
        -> Result<(), BotError>;
}

#[cfg(test)]
mod tests;
