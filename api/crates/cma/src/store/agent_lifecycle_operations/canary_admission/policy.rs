//! The policy and owner grant slots share the existing authority namespaces.

use super::*;
use crate::store::account_deletions;
use mongodb::{bson::Document, ClientSession};

const POLICY_ID: &str = "runtime_canary_policy_v1";
const RETAINED_POLICY_ID: &str = "retained_runtime_canary_policy_v1";
const OWNER_SLOTS: &str = "runtime_canary_slots";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CanaryLane {
    Agent,
    DebianDesktop,
}

impl CanaryLane {
    fn name(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::DebianDesktop => "debian_desktop",
        }
    }

    fn path(self) -> String {
        format!("{OWNER_SLOTS}.{}", self.name())
    }
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanaryPolicy {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    revision: i64,
    open: bool,
    catalog_sha256: String,
    owner_scope_sha256: String,
    fence: i64,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CanaryGrantSlot {
    pub(crate) id: String,
    pub(crate) revision: i64,
    release_key: String,
    manifest_digest: String,
    descriptor_sha256: String,
    consent_sha256: String,
    expires_at: DateTime,
    pub(crate) policy_revision: i64,
    state: CanaryGrantState,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum CanaryGrantState {
    Issued,
    Revoked,
    Consumed { operation_id: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum CanaryWriteError {
    #[error("fresh runtime canary admission is closed")]
    Closed,
    #[error("the runtime canary grant or request does not match current authority")]
    Refused,
    #[error("the runtime canary admission is unresolved")]
    Unresolved,
    #[error("the current account is unavailable for canary admission")]
    OwnerUnavailable,
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("runtime canary persistence failed")]
    Driver(#[from] Error),
}

pub(super) async fn fence_policy(
    database: &Database,
    revision: i64,
    catalog_sha256: &str,
    owner_scope_sha256: &str,
    session: &mut ClientSession,
) -> Result<(), CanaryWriteError> {
    fence_policy_record(
        database,
        POLICY_ID,
        revision,
        catalog_sha256,
        owner_scope_sha256,
        session,
    )
    .await
}

pub(in crate::store) async fn fence_retained_policy(
    database: &Database,
    revision: i64,
    catalog_sha256: &str,
    owner_scope_sha256: &str,
    session: &mut ClientSession,
) -> Result<(), CanaryWriteError> {
    fence_policy_record(
        database,
        RETAINED_POLICY_ID,
        revision,
        catalog_sha256,
        owner_scope_sha256,
        session,
    )
    .await
}

async fn fence_policy_record(
    database: &Database,
    policy_id: &str,
    revision: i64,
    catalog_sha256: &str,
    owner_scope_sha256: &str,
    session: &mut ClientSession,
) -> Result<(), CanaryWriteError> {
    let policies = database.collection::<CanaryPolicy>(crate::store::settings::SETTINGS);
    let updated = policies
        .update_one(
            doc! {
                "_id": policy_id,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": revision,
                "open": true,
                "catalog_sha256": catalog_sha256,
                "owner_scope_sha256": owner_scope_sha256,
            },
            doc! {"$inc": {"fence": 1_i64}},
        )
        .session(session)
        .await?;
    if updated.matched_count == 1 {
        Ok(())
    } else {
        Err(CanaryWriteError::Closed)
    }
}

pub(super) async fn fence_owner(
    database: &Database,
    owner: &SubjectId,
    session: &mut ClientSession,
) -> Result<(), CanaryWriteError> {
    if account_deletions::fence_subject_in_session(database, owner, session).await? {
        Err(CanaryWriteError::OwnerUnavailable)
    } else {
        Ok(())
    }
}

pub(super) async fn slot(
    database: &Database,
    owner: &SubjectId,
    lane: CanaryLane,
    session: &mut ClientSession,
) -> Result<Option<CanaryGrantSlot>, CanaryWriteError> {
    let row = database
        .collection::<Document>(crate::store::sandbox_mongo::SANDBOX_ADMISSIONS)
        .find_one(doc! {"_id": owner.as_str()})
        .projection(doc! { (lane.path()): 1 })
        .session(session)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let Some(slots) = row.get(OWNER_SLOTS) else {
        return Ok(None);
    };
    let Bson::Document(slots) = slots else {
        return Err(invalid_authority().into());
    };
    slots
        .get(lane.name())
        .map(|value| {
            mongodb::bson::deserialize_from_bson(value.clone())
                .map_err(|_| invalid_authority().into())
        })
        .transpose()
}

pub(super) async fn replace_slot(
    database: &Database,
    owner: &SubjectId,
    lane: CanaryLane,
    expected: Option<&CanaryGrantSlot>,
    next: &CanaryGrantSlot,
    session: &mut ClientSession,
) -> Result<(), CanaryWriteError> {
    let key = lane.path();
    let mut filter = doc! {"_id": owner.as_str()};
    if let Some(expected) = expected {
        filter.insert(format!("{key}.id"), &expected.id);
        filter.insert(format!("{key}.revision"), expected.revision);
    } else {
        filter.insert(&key, doc! {"$exists": false});
    }
    let current_expiry = || format!("${key}.expires_at");
    match next.state {
        CanaryGrantState::Issued => {
            let new_grant_is_live = doc! { "$gt": [next.expires_at, "$$NOW"] };
            let expiry = if expected.is_some_and(|grant| grant.state == CanaryGrantState::Issued) {
                doc! { "$and": [
                    new_grant_is_live,
                    doc! { "$lte": [current_expiry(), "$$NOW"] },
                ] }
            } else {
                new_grant_is_live
            };
            filter.insert("$expr", expiry);
        }
        CanaryGrantState::Consumed { .. } => {
            filter.insert("$expr", doc! { "$gt": [current_expiry(), "$$NOW"] });
        }
        CanaryGrantState::Revoked => {}
    }
    let value = mongodb::bson::serialize_to_bson(next).map_err(|_| invalid_authority())?;
    let updated = database
        .collection::<Document>(crate::store::sandbox_mongo::SANDBOX_ADMISSIONS)
        .update_one(filter, doc! {"$set": {key: value}})
        .session(session)
        .await?;
    if updated.matched_count == 1 {
        Ok(())
    } else {
        Err(CanaryWriteError::Refused)
    }
}

impl CanaryPolicy {
    /// Only an explicit operator command changes the policy revision.
    pub(crate) async fn configure(
        database: &Database,
        expected_revision: i64,
        open: bool,
        catalog_sha256: String,
        owner_scope_sha256: String,
    ) -> Result<i64, CanaryWriteError> {
        Self::configure_record(
            database,
            POLICY_ID,
            expected_revision,
            open,
            catalog_sha256,
            owner_scope_sha256,
        )
        .await
    }

    pub(crate) async fn configure_retained(
        database: &Database,
        expected_revision: i64,
        open: bool,
        catalog_sha256: String,
        owner_scope_sha256: String,
    ) -> Result<i64, CanaryWriteError> {
        Self::configure_record(
            database,
            RETAINED_POLICY_ID,
            expected_revision,
            open,
            catalog_sha256,
            owner_scope_sha256,
        )
        .await
    }

    async fn configure_record(
        database: &Database,
        policy_id: &str,
        expected_revision: i64,
        open: bool,
        catalog_sha256: String,
        owner_scope_sha256: String,
    ) -> Result<i64, CanaryWriteError> {
        if expected_revision < 0
            || !canonical_sha256(&catalog_sha256)
            || !canonical_sha256(&owner_scope_sha256)
        {
            return Err(CanaryWriteError::Refused);
        }
        let revision = next_revision(expected_revision)?;
        let collection =
            majority_collection::<CanaryPolicy>(database, crate::store::settings::SETTINGS);
        if expected_revision == 0 {
            let policy = CanaryPolicy {
                id: policy_id.into(),
                document_schema_version: DOCUMENT_SCHEMA_VERSION,
                revision,
                open,
                catalog_sha256,
                owner_scope_sha256,
                fence: 0,
            };
            return match collection.insert_one(policy).await {
                Ok(_) => Ok(revision),
                Err(error) if is_duplicate_key(&error) => Err(CanaryWriteError::Refused),
                Err(error) => Err(error.into()),
            };
        }
        let updated = collection
            .update_one(
                doc! {
                    "_id": policy_id,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "revision": expected_revision,
                },
                doc! {"$set": {
                    "revision": revision,
                    "open": open,
                    "catalog_sha256": catalog_sha256,
                    "owner_scope_sha256": owner_scope_sha256,
                }},
            )
            .await?;
        if updated.matched_count == 1 {
            Ok(revision)
        } else {
            Err(CanaryWriteError::Refused)
        }
    }
}

impl CanaryGrantSlot {
    pub(super) fn consume(
        &self,
        grant_id: &str,
        operation_id: &str,
        digest: &str,
        release: &FrozenRelease,
        policy_revision: i64,
    ) -> Result<Self, CanaryWriteError> {
        let descriptor =
            layer2_core::profiles::content_digest(release).map_err(|_| invalid_authority())?;
        if self.id != grant_id
            || self.release_key != release.release_key
            || self.manifest_digest != digest
            || self.descriptor_sha256 != descriptor
            || !canonical_sha256(&self.consent_sha256)
            || self.policy_revision != policy_revision
            || self.state != CanaryGrantState::Issued
        {
            return Err(CanaryWriteError::Refused);
        }
        Ok(Self {
            revision: next_revision(self.revision)?,
            state: CanaryGrantState::Consumed {
                operation_id: operation_id.to_owned(),
            },
            ..self.clone()
        })
    }

    pub(super) fn receipt(
        &self,
        owner: &SubjectId,
        operation_id: &str,
        release: &FrozenRelease,
    ) -> Result<CanaryAdmission, StoreError> {
        Ok(CanaryAdmission {
            grant_id: self.id.clone(),
            policy_revision: self.policy_revision,
            consent_sha256: self.consent_sha256.clone(),
            grant_expires_at: self.expires_at,
            release: release.clone(),
            profiles: CanaryProfiles::for_operation(owner, operation_id)?,
            fingerprint: String::new(),
        })
    }
}

fn next_revision(revision: i64) -> Result<i64, CanaryWriteError> {
    revision.checked_add(1).ok_or(CanaryWriteError::Refused)
}

pub(super) fn canonical_sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

mod commands;
pub(crate) use commands::{FreshCanaryGrant, PrepareCanaryGrant, RevokeCanaryGrant};
