//! Committed canary authority travels with the immutable lifecycle input.

use super::*;
use layer2_core::profiles::{
    private_agent_profile_id, private_sandbox_profile_id, private_workspace_profile_id,
    AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef,
};
use layer3_adapter::{RuntimeImage, SandboxCapacity};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CanaryAdmission {
    grant_id: String,
    policy_revision: i64,
    consent_sha256: String,
    grant_expires_at: DateTime,
    release: FrozenRelease,
    profiles: CanaryProfiles,
    fingerprint: String,
}

impl std::fmt::Debug for CanaryAdmission {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CanaryAdmission")
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenRelease {
    release_key: String,
    image: RuntimeImage,
    config_digest: String,
    source_revision: String,
    publication_evidence_sha256: String,
    capacity: SandboxCapacity,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanaryProfiles {
    pub(crate) agent: AgentProfileRef,
    pub(crate) workspace: WorkspaceProfileRef,
    pub(crate) sandbox: SandboxProfileRef,
}

impl CanaryProfiles {
    fn for_operation(owner: &SubjectId, id: &str) -> Result<Self, StoreError> {
        let key = |kind| canary_key::profile_key(id, kind);
        let agent =
            private_agent_profile_id(owner, &key("agent")).map_err(|_| invalid_authority())?;
        let workspace = private_workspace_profile_id(owner, &key("workspace"))
            .map_err(|_| invalid_authority())?;
        let sandbox =
            private_sandbox_profile_id(owner, &key("sandbox")).map_err(|_| invalid_authority())?;
        Ok(Self {
            agent: AgentProfileRef::new(agent, 1),
            workspace: WorkspaceProfileRef::new(workspace, 1),
            sandbox: SandboxProfileRef::new(sandbox, 1),
        })
    }
}

impl LifecycleRecord {
    /// The majority-acknowledged claim returns the proof and input together.
    /// A later grant expiry or policy change cannot alter admitted work.
    pub(crate) fn verify_canary_worker(&self) -> Result<Option<CanaryProfiles>, StoreError> {
        let Some(admission) = &self.canary_admission else {
            return if canary_key::is_reserved(&self.key) {
                Err(invalid_authority())
            } else {
                Ok(None)
            };
        };
        schema::require_current(self.document_schema_version)?;
        if canary_key::operation_id(&self.key) != Some(self.id.as_str())
            || admission.fingerprint != admission.fingerprint(self)?
            || admission.profiles
                != CanaryProfiles::for_operation(&SubjectId::new(&self.owner), &self.id)?
            || self.input.resolved_image.as_deref() != Some(admission.release.image.uri.as_str())
            || self.input.capacity != Some(admission.release.capacity)
            || self.input.request.workspace_id.is_some()
            || self.input.request.sandbox_id.is_some()
            || self.input.request.repository.is_some()
        {
            return Err(invalid_authority());
        }
        Ok(Some(admission.profiles.clone()))
    }
}

impl CanaryAdmission {
    fn fingerprint(&self, record: &LifecycleRecord) -> Result<String, StoreError> {
        layer2_core::profiles::content_digest(&(
            "runtime-canary-admission-v1",
            &self.grant_id,
            self.policy_revision,
            &self.consent_sha256,
            self.grant_expires_at,
            &self.release,
            &self.profiles,
            &record.owner,
            &record.id,
            &record.key,
            &record.input,
        ))
        .map_err(|_| invalid_authority())
    }
}

fn invalid_authority() -> StoreError {
    StoreError::Malformed("canary operation is missing its committed authority".into())
}

#[cfg(test)]
impl MongoAgentLifecycleStore {
    /// Simulate the durable inputs of a later compatible admission writer.
    pub(crate) async fn install_canary_worker_fixture(
        &self,
        record: &mut LifecycleRecord,
        image: RuntimeImage,
    ) -> CanaryProfiles {
        record.key = canary_key::fixture_key(&record.id);
        record.input.resolved_image = Some(image.uri.clone());
        let profiles =
            CanaryProfiles::for_operation(&SubjectId::new(&record.owner), &record.id).unwrap();
        let mut admission = CanaryAdmission {
            grant_id: "grant-fixture".into(),
            policy_revision: 1,
            consent_sha256: "d".repeat(64),
            grant_expires_at: DateTime::from_millis(0),
            release: FrozenRelease {
                release_key: "agent".into(),
                image,
                config_digest: format!("sha256:{}", "a".repeat(64)),
                source_revision: "b".repeat(40),
                publication_evidence_sha256: "c".repeat(64),
                capacity: record.input.capacity.unwrap(),
            },
            profiles: profiles.clone(),
            fingerprint: String::new(),
        };
        admission.fingerprint = admission.fingerprint(record).unwrap();
        record.canary_admission = Some(admission);
        self.operations
            .replace_one(doc! {"_id": &record.id}, &*record)
            .await
            .unwrap();
        profiles
    }
}
