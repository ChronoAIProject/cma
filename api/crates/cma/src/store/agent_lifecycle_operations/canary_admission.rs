//! Committed canary authority travels with the immutable lifecycle input.

use super::*;
use sha2::{Digest, Sha256};

mod policy;
pub(in crate::store) use policy::fence_retained_policy;
pub(crate) use policy::{
    CanaryLane, CanaryPolicy, CanaryWriteError, FreshCanaryGrant, PrepareCanaryGrant,
    RevokeCanaryGrant,
};
mod writer;
pub(crate) use writer::{FreshCanaryRedemption, PreparedCanaryRedemption};
#[cfg(test)]
mod writer_tests;
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
pub(crate) struct FrozenRelease {
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

pub(in crate::store) const SANDBOX_PROFILE_ID: &str = "canary_admission.profiles.sandbox.id";
pub(in crate::store) const MANIFEST_DIGEST: &str = "canary_manifest_digest";
const REQUEST_KEY_DIGEST: &str = "canary_redemption_key.sha256";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CanaryRedemptionKey {
    sha256: String,
    lane: policy::CanaryLane,
    proof: String,
}

impl CanaryRedemptionKey {
    fn new(
        request_key: &str,
        lane: policy::CanaryLane,
        admission: &CanaryAdmission,
    ) -> Result<Self, StoreError> {
        let proof = Self::proof(request_key, lane, admission)?;
        Ok(Self {
            sha256: request_key.to_owned(),
            lane,
            proof,
        })
    }

    fn proof(
        request_key: &str,
        lane: policy::CanaryLane,
        admission: &CanaryAdmission,
    ) -> Result<String, StoreError> {
        if !policy::canonical_sha256(request_key) {
            return Err(invalid_authority());
        }
        layer2_core::profiles::content_digest(&(
            "runtime-canary-redemption-key-v1",
            request_key,
            lane,
            &admission.fingerprint,
        ))
        .map_err(|_| invalid_authority())
    }
}

pub(in crate::store) fn image_digest(image: &str) -> Option<String> {
    let (_, digest) = image.rsplit_once("@sha256:")?;
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

pub(super) async fn ensure_admission_indexes(
    operations: &Collection<LifecycleRecord>,
) -> Result<(), StoreError> {
    operations
        .create_index(
            IndexModel::builder()
                .keys(doc! { SANDBOX_PROFILE_ID: 1 })
                .options(
                    IndexOptions::builder()
                        .name("agent_lifecycle_canary_sandbox_profile".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! { SANDBOX_PROFILE_ID: {"$exists": true} })
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(driver)?;
    operations
        .create_index(
            IndexModel::builder()
                .keys(doc! { MANIFEST_DIGEST: 1, "owner": 1 })
                .options(
                    IndexOptions::builder()
                        .name("agent_lifecycle_canary_manifest_owner".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! { MANIFEST_DIGEST: {"$exists": true} })
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(driver)?;
    operations
        .create_index(
            IndexModel::builder()
                .keys(doc! { "owner": 1, REQUEST_KEY_DIGEST: 1 })
                .options(
                    IndexOptions::builder()
                        .name("agent_lifecycle_canary_owner_request".to_owned())
                        .unique(true)
                        .partial_filter_expression(doc! { REQUEST_KEY_DIGEST: {"$exists": true} })
                        .build(),
                )
                .build(),
        )
        .await
        .map_err(driver)?;
    Ok(())
}

/// Claim the permanent operation-to-Sandbox binding under the same transaction
/// that inserts the Sandbox. The binding survives retirement and purge.
pub(in crate::store) async fn bind_sandbox(
    operations: &Collection<LifecycleRecord>,
    lease: &LifecycleLease,
    sandbox: &layer2_core::SandboxId,
    session: &mut mongodb::ClientSession,
) -> Result<bool, Error> {
    let mut filter = lease.filter();
    filter.extend(doc! {
        "canary_admission": {"$exists": true},
        "sandbox_id": Bson::Null,
    });
    let result = operations
        .update_one(
            filter,
            vec![doc! {"$set": {"sandbox_id": sandbox.as_str(), "updated_at": "$$NOW"}}],
        )
        .session(session)
        .await?;
    Ok(result.matched_count == 1)
}

impl From<&crate::config::runtime_canary::Release> for FrozenRelease {
    fn from(release: &crate::config::runtime_canary::Release) -> Self {
        Self {
            release_key: release.release_key.clone(),
            image: release.image.clone(),
            config_digest: release.config_digest.clone(),
            source_revision: release.source_revision.clone(),
            publication_evidence_sha256: release.publication_evidence_sha256.clone(),
            capacity: release.capacity,
        }
    }
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
            return if canary_key::is_reserved(&self.key) || self.canary_manifest_digest.is_some() {
                Err(invalid_authority())
            } else {
                Ok(None)
            };
        };
        schema::require_current(self.document_schema_version)?;
        let digest = image_digest(&admission.release.image.uri).ok_or_else(invalid_authority)?;
        if self.canary_manifest_digest.as_deref() != Some(digest.as_str())
            || canary_key::operation_id(&self.key) != Some(self.id.as_str())
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
        if let Some(key) = &self.canary_redemption_key {
            if key.proof != CanaryRedemptionKey::proof(&key.sha256, key.lane, admission)? {
                return Err(invalid_authority());
            }
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

/// Recover the immutable provider descriptor from the operation that bound this home.
pub(in crate::store) async fn runtime_image(
    database: &Database,
    sandbox: &layer2_core::sandbox::Sandbox,
) -> Result<Option<RuntimeImage>, layer2_core::sandbox::SandboxStoreError> {
    use layer2_core::sandbox::{SandboxOrigin, SandboxStoreError};
    let SandboxOrigin::Lifecycle { operation_id } = &sandbox.origin else {
        return Ok(None);
    };
    if sandbox.profile.content.runtime_image.is_none() {
        return Ok(None);
    }
    let operations = majority_collection::<LifecycleRecord>(database, COLLECTION);
    let Some(record) = operations
        .find_one(doc! {"_id": operation_id})
        .await
        .map_err(|_| {
            SandboxStoreError::Unavailable("canary runtime authority read failed".into())
        })?
    else {
        return Ok(None);
    };
    let bad =
        || SandboxStoreError::Corrupt("canary runtime authority does not match the Sandbox".into());
    let Some(profiles) = record.verify_canary_worker().map_err(|_| bad())? else {
        return Ok(None);
    };
    let authority = record.canary_admission.as_ref().ok_or_else(bad)?;
    if record.owner != sandbox.owner.as_str()
        || record.sandbox_id.as_deref() != Some(sandbox.id.as_str())
        || sandbox.profile.id != profiles.sandbox.id
        || sandbox.profile.revision != profiles.sandbox.revision
        || sandbox.profile.content.runtime_image.as_deref()
            != Some(authority.release.image.uri.as_str())
        || sandbox.profile.content.cpu_millis != authority.release.capacity.cpu_millis
        || sandbox.profile.content.memory_mib != authority.release.capacity.memory_mib
    {
        return Err(bad());
    }
    Ok(Some(authority.release.image.clone()))
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
        record.key = canary_key::admission_key(&record.id);
        record.input.resolved_image = Some(image.uri.clone());
        record.canary_manifest_digest = image_digest(&image.uri);
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
