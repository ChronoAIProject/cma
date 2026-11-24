//! The worker's compatible reader for previously committed canary grants.

use super::*;
use layer2_core::profiles::{
    profile_id_for_owner, AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef,
};
use layer2_core::{AgentProfileId, SandboxProfileId, WorkspaceProfileId};
use layer3_adapter::{RuntimeImage, SandboxCapacity};

pub(in crate::store) const COLLECTION: &str = "runtime_canary_grants";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CanaryGrant {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    owner: SubjectId,
    release: FrozenRelease,
    policy_revision: i64,
    consent_sha256: String,
    expires_at: DateTime,
    revision: i64,
    state: GrantState,
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum GrantState {
    Issued,
    Revoked,
    Consumed {
        operation_id: String,
        admission_sha256: String,
        profiles: CanaryProfiles,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanaryProfiles {
    pub(crate) agent: AgentProfileRef,
    pub(crate) workspace: WorkspaceProfileRef,
    pub(crate) sandbox: SandboxProfileRef,
}

pub(super) async fn open(database: &Database) -> Result<Collection<CanaryGrant>, StoreError> {
    let collection = majority_collection(database, COLLECTION);
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {"state.operation_id": 1})
                .options(
                    IndexOptions::builder()
                        .name("runtime_canary_consumed_operation".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! {"state.kind": "consumed"})
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(driver)?;
    collection
        .create_index(IndexModel::builder().keys(doc! {"owner": 1}).build())
        .await
        .map_err(driver)?;
    Ok(collection)
}

impl CanaryProfiles {
    fn for_operation(owner: &SubjectId, id: &str) -> Result<Self, StoreError> {
        let profile_id = |kind| {
            profile_id_for_owner(owner, kind, &canary_key::profile_key(id, kind))
                .map_err(|_| invalid_authority())
        };
        Ok(Self {
            agent: AgentProfileRef::new(AgentProfileId::new(profile_id("agent")?), 1),
            workspace: WorkspaceProfileRef::new(
                WorkspaceProfileId::new(profile_id("workspace")?),
                1,
            ),
            sandbox: SandboxProfileRef::new(SandboxProfileId::new(profile_id("sandbox")?), 1),
        })
    }
}

impl MongoAgentLifecycleStore {
    /// Admission flags govern future grants. Workers resolve previously
    /// committed authority from the grant and immutable lifecycle input.
    pub(crate) async fn verify_canary_worker(
        &self,
        record: &LifecycleRecord,
    ) -> Result<Option<CanaryProfiles>, StoreError> {
        if !canary_key::is_reserved(&record.key) {
            return Ok(None);
        }
        if canary_key::operation_id(&record.key) != Some(record.id.as_str()) {
            return Err(invalid_authority());
        }
        let grant = self
            .canary_grants
            .find_one(doc! {"state.operation_id": &record.id, "state.kind": "consumed"})
            .await
            .map_err(|_| StoreError::Unavailable("canary grant read failed".into()))?
            .ok_or_else(invalid_authority)?;
        grant.verify_input(record).map(Some)
    }
}

impl CanaryGrant {
    fn verify_input(&self, record: &LifecycleRecord) -> Result<CanaryProfiles, StoreError> {
        schema::require_current(self.document_schema_version)?;
        schema::require_current(record.document_schema_version)?;
        let GrantState::Consumed {
            operation_id,
            admission_sha256,
            profiles,
        } = &self.state
        else {
            return Err(invalid_authority());
        };
        if self.owner.as_str() != record.owner
            || operation_id != &record.id
            || admission_sha256 != &self.fingerprint(record)?
            || *profiles != CanaryProfiles::for_operation(&self.owner, &record.id)?
            || record.input.resolved_image.as_deref() != Some(self.release.image.uri.as_str())
            || record.input.capacity != Some(self.release.capacity)
            || record.input.request.workspace_id.is_some()
            || record.input.request.sandbox_id.is_some()
            || record.input.request.repository.is_some()
        {
            return Err(invalid_authority());
        }
        Ok(profiles.clone())
    }

    fn fingerprint(&self, record: &LifecycleRecord) -> Result<String, StoreError> {
        layer2_core::profiles::content_digest(&(
            "runtime-canary-admission-v1",
            &self.id,
            &self.owner,
            &self.release,
            self.policy_revision,
            &self.consent_sha256,
            self.expires_at,
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
        let owner = SubjectId::new(&record.owner);
        let profiles = CanaryProfiles::for_operation(&owner, &record.id).unwrap();
        let mut grant = CanaryGrant {
            id: "grant-fixture".into(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            owner,
            release: FrozenRelease {
                release_key: "agent".into(),
                image,
                config_digest: format!("sha256:{}", "a".repeat(64)),
                source_revision: "b".repeat(40),
                publication_evidence_sha256: "c".repeat(64),
                capacity: record.input.capacity.unwrap(),
            },
            policy_revision: 1,
            consent_sha256: "d".repeat(64),
            // A committed receipt must remain usable after the grant expires.
            expires_at: DateTime::from_millis(0),
            revision: 2,
            state: GrantState::Issued,
        };
        grant.state = GrantState::Consumed {
            operation_id: record.id.clone(),
            admission_sha256: grant.fingerprint(record).unwrap(),
            profiles: profiles.clone(),
        };
        self.operations
            .replace_one(doc! {"_id": &record.id}, &*record)
            .await
            .unwrap();
        self.canary_grants.insert_one(grant).await.unwrap();
        profiles
    }
}
