use async_trait::async_trait;
use layer3_adapter::HomeVolumeAttachment;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::sandbox::store::{
    RuntimeActivitySourceContext, RuntimeMigrationClearance, SandboxStoreError,
};
use crate::sandbox::{RuntimeBinding, Sandbox, SandboxCustomRuntimeMigration};
use crate::{SandboxId, SandboxProfileId, SubjectId};

pub const MAX_RUNTIME_MIGRATION_BUNDLE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_RUNTIME_MIGRATION_ENTRIES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMigrationTarget {
    pub repository: String,
    pub image: String,
}

/// Installed independently of evidence by the deployment operator. A bundle
/// cannot qualify its own producer, writer set, source or approved image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMigrationPolicy {
    pub schema_version: u32,
    pub policy_id: String,
    pub deployment_id: String,
    pub scoped_sandboxes: Vec<SandboxId>,
    pub targets: Vec<RuntimeMigrationTarget>,
    pub qualified_sources: Vec<RuntimeActivitySourceContext>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMigrationIdentity {
    pub sandbox_id: SandboxId,
    pub owner: SubjectId,
    pub revision: u64,
    pub profile_id: SandboxProfileId,
    pub profile_revision: u32,
    pub profile_digest: String,
    pub home: HomeVolumeAttachment,
    pub runtime: Option<RuntimeBinding>,
    pub effective_image: String,
}

impl RuntimeMigrationIdentity {
    pub fn capture(sandbox: &Sandbox) -> Result<Self, SandboxStoreError> {
        sandbox.validate().map_err(|_| invalid())?;
        Ok(Self {
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            revision: sandbox.revision,
            profile_id: sandbox.profile.id.clone(),
            profile_revision: sandbox.profile.revision,
            profile_digest: sandbox.profile.digest.clone(),
            home: sandbox
                .lifecycle
                .home
                .as_ref()
                .and_then(|home| home.attachment())
                .ok_or_else(invalid)?,
            runtime: sandbox.retained_runtime_binding().cloned(),
            effective_image: sandbox
                .effective_custom_runtime_image()
                .or_else(|| {
                    sandbox
                        .retained_runtime_binding()
                        .map(|binding| binding.runtime_version.as_str())
                })
                .ok_or_else(invalid)?
                .to_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMigrationAuthorityEntry {
    pub authority_id: String,
    pub request_key: String,
    pub identity: RuntimeMigrationIdentity,
    pub target_image: String,
    pub custom_migration: Option<SandboxCustomRuntimeMigration>,
    pub clearance: RuntimeMigrationClearance,
    #[serde(default)]
    pub observation_reference: Option<String>,
    pub issued_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeMigrationBundle {
    pub schema_version: u32,
    pub policy_id: String,
    pub entries: Vec<RuntimeMigrationAuthorityEntry>,
}

impl RuntimeMigrationPolicy {
    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        if self.schema_version != 1
            || !label(&self.policy_id)
            || !label(&self.deployment_id)
            || self.scoped_sandboxes.is_empty()
            || self.scoped_sandboxes.len() > MAX_RUNTIME_MIGRATION_ENTRIES
            || self.targets.is_empty()
            || self.targets.len() > 16
            || self.qualified_sources.len() > 16
        {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        for id in &self.scoped_sandboxes {
            if !label(id.as_str()) || !ids.insert(id.as_str()) {
                return Err(invalid());
            }
        }
        let mut repositories = BTreeSet::new();
        for target in &self.targets {
            if repository(&target.image) != Some(target.repository.as_str())
                || !repositories.insert(&target.repository)
            {
                return Err(invalid());
            }
        }
        for (index, source) in self.qualified_sources.iter().enumerate() {
            if self.qualified_sources[..index].contains(source) {
                return Err(invalid());
            }
        }
        Ok(())
    }

    pub fn includes(&self, id: &SandboxId) -> bool {
        self.scoped_sandboxes.contains(id)
    }

    pub fn validate_entry(
        &self,
        entry: &RuntimeMigrationAuthorityEntry,
        now_ms: i64,
    ) -> Result<(), SandboxStoreError> {
        self.validate()?;
        if !self.includes(&entry.identity.sandbox_id)
            || !label(&entry.authority_id)
            || !label(&entry.request_key)
            || entry.identity.revision == 0
            || entry.issued_at_ms > now_ms
            || entry.expires_at_ms < now_ms
            || entry.expires_at_ms < entry.issued_at_ms
            || entry.expires_at_ms.saturating_sub(entry.issued_at_ms) > 30_000
            || !self.targets.iter().any(|target| {
                target.image == entry.target_image
                    && repository(&entry.identity.effective_image)
                        == Some(target.repository.as_str())
            })
        {
            return Err(invalid());
        }
        if let RuntimeMigrationClearance::Observed { proof } = &entry.clearance {
            if !self.qualified_sources.contains(proof.source_context()) {
                return Err(invalid());
            }
            proof.validate_at(now_ms)?;
        }
        if let Some(migration) = &entry.custom_migration {
            if migration.origin.sandbox_id != entry.identity.sandbox_id
                || migration.request_key != entry.request_key
                || migration.target_image != entry.target_image
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}

impl RuntimeMigrationBundle {
    pub fn parse(bytes: &[u8], policy: &RuntimeMigrationPolicy) -> Result<Self, SandboxStoreError> {
        if bytes.len() > MAX_RUNTIME_MIGRATION_BUNDLE_BYTES {
            return Err(invalid());
        }
        let bundle: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        policy.validate()?;
        if bundle.schema_version != 1
            || bundle.policy_id != policy.policy_id
            || bundle.entries.len() > MAX_RUNTIME_MIGRATION_ENTRIES
        {
            return Err(invalid());
        }
        let mut ids = BTreeSet::new();
        let mut keys = BTreeSet::new();
        for entry in &bundle.entries {
            if !policy.includes(&entry.identity.sandbox_id)
                || !ids.insert(entry.identity.sandbox_id.as_str())
                || !keys.insert(&entry.authority_id)
            {
                return Err(invalid());
            }
        }
        Ok(bundle)
    }
}

#[async_trait]
pub trait RuntimeMigrationAuthority: Send + Sync {
    fn policy(&self) -> &RuntimeMigrationPolicy;
    /// Reload through the trusted server publication boundary; no HTTP paths.
    async fn resolve(
        &self,
        sandbox_id: &SandboxId,
    ) -> Result<RuntimeMigrationAuthorityEntry, SandboxStoreError>;
}

pub(crate) fn repository(image: &str) -> Option<&str> {
    if image.len() > 512 || !crate::profiles::valid_runtime_image(image) {
        return None;
    }
    let (repository, _) = image.rsplit_once("@sha256:")?;
    (!repository.rsplit('/').next()?.contains(':')).then_some(repository)
}
fn label(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}
fn invalid() -> SandboxStoreError {
    SandboxStoreError::InvalidTransaction("runtime migration authority is invalid")
}
