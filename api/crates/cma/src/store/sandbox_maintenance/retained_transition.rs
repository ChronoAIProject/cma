//! Consume one retained-home grant in the transaction that reserves its successor.
use layer2_core::sandbox::{RetainedRuntimeTransition, RetainedRuntimeTransitionRequest};
use mongodb::bson::{self, doc, DateTime, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::{ClientSession, Database};
use serde::{Deserialize, Serialize};

use super::super::account_deletions::{erasing_in_session, fence_subject_in_session};
use super::super::sandbox_mongo::{is_confirmed_body_retry, start_transaction};
use super::*;

const GRANTS: &str = "retained_runtime_grants";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RetainedRuntimeGrant {
    pub(crate) request: RetainedRuntimeTransitionRequest,
    pub(crate) release_key: String,
    pub(crate) catalog_sha256: String,
    pub(crate) owner_scope_sha256: String,
    pub(crate) issued_by: layer2_core::SubjectId,
    pub(crate) consent_sha256: String,
    pub(crate) custody_sha256: String,
    pub(crate) expires_at: DateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) receipt: Option<RetainedRuntimeTransition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) aborted_at: Option<DateTime>,
}

/// Fresh evidence resolved under the configured retained-release policy.
pub(crate) struct RetainedRuntimeGrantPolicy<'a> {
    pub(crate) release_key: &'a str,
    pub(crate) catalog_sha256: &'a str,
    pub(crate) owner_scope_sha256: &'a str,
    pub(crate) target_image: &'a str,
    pub(crate) target_config_digest: &'a str,
}

#[path = "retained_transition_abort.rs"]
mod abort;

impl MongoSandboxStore {
    pub(crate) async fn read_retained_runtime_grant(
        &self,
        owner: &layer2_core::SubjectId,
        id: &str,
    ) -> Result<RetainedRuntimeGrant, SandboxStoreError> {
        let path = grant_path(id)?;
        let row = self
            .sandbox_admissions
            .find_one(doc! {"_id": owner.as_str()})
            .projection(doc! {(&path): 1})
            .await
            .map_err(unavailable)?
            .ok_or(SandboxStoreError::NotFound)?;
        let value = row
            .get_document(GRANTS)
            .ok()
            .and_then(|grants| grants.get(id))
            .ok_or(SandboxStoreError::NotFound)?;
        let grant: RetainedRuntimeGrant =
            bson::deserialize_from_bson(value.clone()).map_err(|_| unresolved())?;
        if grant.request.owner != *owner || grant.request.grant_id != id {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(grant)
    }

    /// Persist an explicitly scoped operator grant before the owner can redeem it.
    pub(crate) async fn issue_retained_runtime_grant(
        &self,
        database: &Database,
        grant: &RetainedRuntimeGrant,
    ) -> Result<(), SandboxStoreError> {
        if grant.receipt.is_some() || grant.aborted_at.is_some() {
            return Err(SandboxStoreError::Conflict);
        }
        let path = grant_path(&grant.request.grant_id)?;
        if let Ok(mut existing) = self
            .read_retained_runtime_grant(&grant.request.owner, &grant.request.grant_id)
            .await
        {
            existing.receipt = None;
            existing.aborted_at = None;
            return if existing == *grant {
                Ok(())
            } else {
                Err(SandboxStoreError::Conflict)
            };
        }
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        start_transaction(&mut session).await.map_err(unavailable)?;
        let prepared = async {
            if fence_subject_in_session(database, &grant.request.owner, &mut session)
                .await
                .map_err(ApplyError::Driver)?
            {
                return Err(refused());
            }
            fence_policy(
                database,
                &grant.request,
                &grant.catalog_sha256,
                &grant.owner_scope_sha256,
                &mut session,
            )
            .await?;
            let value = bson::serialize_to_bson(grant).map_err(|_| refused())?;
            let updated = self
                .sandbox_admissions
                .update_one(
                    doc! {
                        "_id": grant.request.owner.as_str(),
                        (&path): {"$exists": false},
                        "$expr": {"$and": [
                            {"$gt": [grant.expires_at, "$$NOW"]},
                            {"$lt": [
                                {"$size": {"$objectToArray": {"$ifNull": [
                                    format!("${GRANTS}"), doc! {}
                                ]}}},
                                128_i32
                            ]}
                        ]},
                    },
                    doc! {"$set": {(&path): value}},
                )
                .session(&mut session)
                .await
                .map_err(ApplyError::Driver)?;
            if updated.modified_count != 1 {
                return Err(refused());
            }
            Ok(())
        }
        .await;
        if let Err(error) = prepared {
            let _ = session.abort_guarded().await;
            return Err(public_error(error));
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => Ok(()),
            CommitOutcome::Failed(error) => Err(unavailable(error)),
            CommitOutcome::Unknown(_) | CommitOutcome::Retry(_) => Err(unresolved()),
        }
    }

    /// Replay reads the frozen receipt before checking the current release catalog.
    /// The caller must already have authenticated the owner of the request.
    pub(crate) async fn redeem_retained_runtime_grant(
        &self,
        database: &Database,
        request: RetainedRuntimeTransitionRequest,
        fresh_policy: Option<RetainedRuntimeGrantPolicy<'_>>,
    ) -> Result<RetainedRuntimeTransition, SandboxStoreError> {
        let path = grant_path(&request.grant_id)?;
        let operation = MaintenanceRequest::RetainedTransition {
            request: Box::new(request.clone()),
            requested_at: now_rfc3339(),
        };
        let mut retries = retry::Retries::new();
        while retries.next().await {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let applied = self
                .redeem_retained_runtime_grant_once(
                    database,
                    &path,
                    &request,
                    fresh_policy.as_ref(),
                    &operation,
                    &mut session,
                )
                .await;
            let receipt = match applied {
                Ok((receipt, true)) => receipt,
                Ok((receipt, false)) => {
                    session.abort_guarded().await.map_err(unavailable)?;
                    return Ok(receipt);
                }
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    match error {
                        ApplyError::Driver(error) if is_confirmed_body_retry(&error) => continue,
                        ApplyError::Domain(error) => return Err(error),
                        ApplyError::Driver(error) => return Err(unavailable(error)),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Failed(error) => return Err(unavailable(error)),
                CommitOutcome::Unknown(_) => {
                    let at = session.operation_time().ok_or_else(unresolved)?;
                    return self.prove_retained_runtime_grant(&path, &receipt, at).await;
                }
            }
        }
        Err(unresolved())
    }

    async fn redeem_retained_runtime_grant_once(
        &self,
        database: &Database,
        path: &str,
        request: &RetainedRuntimeTransitionRequest,
        fresh_policy: Option<&RetainedRuntimeGrantPolicy<'_>>,
        operation: &MaintenanceRequest,
        session: &mut ClientSession,
    ) -> Result<(RetainedRuntimeTransition, bool), ApplyError> {
        if erasing_in_session(database, &request.owner, session)
            .await
            .map_err(ApplyError::Driver)?
        {
            return Err(refused());
        }
        let grant = self.retained_runtime_grant(path, request, session).await?;
        if let Some(receipt) = grant.receipt {
            if receipt.request != *request {
                return Err(refused());
            }
            return Ok((receipt, false));
        }
        if grant.aborted_at.is_some() {
            return Err(refused());
        }
        let fresh = fresh_policy.ok_or_else(refused)?;
        if grant.release_key != fresh.release_key
            || grant.catalog_sha256 != fresh.catalog_sha256
            || grant.owner_scope_sha256 != fresh.owner_scope_sha256
            || request.target_image != fresh.target_image
            || request.target_config_digest != fresh.target_config_digest
            || fence_subject_in_session(database, &request.owner, session)
                .await
                .map_err(ApplyError::Driver)?
        {
            return Err(refused());
        }
        fence_policy(
            database,
            request,
            fresh.catalog_sha256,
            fresh.owner_scope_sha256,
            session,
        )
        .await?;
        let applied = transaction::apply_with_sources(
            self,
            operation,
            session,
            None,
            IntegrityFailurePolicy::ReportOnly,
        )
        .await?;
        let receipt = applied
            .sandbox
            .admitted_retained_runtime_transition()
            .filter(|receipt| applied.event.is_some() && receipt.request == *request)
            .cloned()
            .ok_or_else(refused)?;
        let frozen = bson::serialize_to_bson(&grant).map_err(|_| refused())?;
        let result = self.sandbox_admissions.update_one(
            doc! {
                "_id": request.owner.as_str(),
                (path): frozen,
                "$expr": {"$gt": [format!("${path}.expires_at"), "$$NOW"]},
            },
            doc! {"$set": {
                (format!("{path}.receipt")): bson::serialize_to_bson(&receipt).map_err(|_| refused())?,
            }},
        ).session(session).await.map_err(ApplyError::Driver)?;
        if result.modified_count != 1 {
            return Err(refused());
        }
        Ok((receipt, true))
    }

    async fn retained_runtime_grant(
        &self,
        path: &str,
        request: &RetainedRuntimeTransitionRequest,
        session: &mut ClientSession,
    ) -> Result<RetainedRuntimeGrant, ApplyError> {
        let row = self
            .sandbox_admissions
            .find_one(doc! {"_id": request.owner.as_str()})
            .projection(doc! {(path): 1})
            .session(session)
            .await
            .map_err(ApplyError::Driver)?
            .ok_or_else(refused)?;
        let key = path
            .strip_prefix(&format!("{GRANTS}."))
            .ok_or_else(refused)?;
        let value = row
            .get_document(GRANTS)
            .ok()
            .and_then(|grants| grants.get(key))
            .ok_or_else(refused)?;
        let grant: RetainedRuntimeGrant =
            bson::deserialize_from_bson(value.clone()).map_err(|_| refused())?;
        if grant.request != *request {
            return Err(refused());
        }
        Ok(grant)
    }

    async fn prove_retained_runtime_grant(
        &self,
        path: &str,
        expected: &RetainedRuntimeTransition,
        at: Timestamp,
    ) -> Result<RetainedRuntimeTransition, SandboxStoreError> {
        let mut session = self
            .client
            .start_session()
            .snapshot(true)
            .snapshot_time(at)
            .await
            .map_err(unavailable)?;
        let row = self
            .sandbox_admissions
            .find_one(doc! {"_id": expected.request.owner.as_str()})
            .read_concern(ReadConcern::snapshot())
            .projection(doc! {(path): 1})
            .session(&mut session)
            .await
            .map_err(unavailable)?
            .ok_or_else(unresolved)?;
        let key = path
            .strip_prefix(&format!("{GRANTS}."))
            .ok_or_else(unresolved)?;
        let value = row
            .get_document(GRANTS)
            .ok()
            .and_then(|grants| grants.get(key))
            .ok_or_else(unresolved)?;
        let grant: RetainedRuntimeGrant =
            bson::deserialize_from_bson(value.clone()).map_err(|_| unresolved())?;
        if grant.request == expected.request && grant.receipt.as_ref() == Some(expected) {
            Ok(expected.clone())
        } else {
            Err(unresolved())
        }
    }
}

async fn fence_policy(
    database: &Database,
    request: &RetainedRuntimeTransitionRequest,
    catalog_sha256: &str,
    owner_scope_sha256: &str,
    session: &mut ClientSession,
) -> Result<(), ApplyError> {
    let revision = i64::try_from(request.policy_revision).map_err(|_| refused())?;
    crate::store::runtime_canary::fence_retained_policy(
        database,
        revision,
        catalog_sha256,
        owner_scope_sha256,
        session,
    )
    .await
    .map_err(|error| match error {
        crate::store::runtime_canary::CanaryWriteError::Driver(error) => ApplyError::Driver(error),
        _ => refused(),
    })
}

fn public_error(error: ApplyError) -> SandboxStoreError {
    match error {
        ApplyError::Domain(error) => error,
        ApplyError::Driver(error) => unavailable(error),
    }
}

fn grant_path(id: &str) -> Result<String, SandboxStoreError> {
    let grant = uuid::Uuid::parse_str(id).map_err(|_| SandboxStoreError::Conflict)?;
    if grant.simple().to_string() != id {
        return Err(SandboxStoreError::Conflict);
    }
    Ok(format!("{GRANTS}.{id}"))
}

fn refused() -> ApplyError {
    ApplyError::Domain(SandboxStoreError::InvalidTransaction(
        "retained runtime grant refused",
    ))
}

fn unresolved() -> SandboxStoreError {
    SandboxStoreError::Unavailable("retained runtime grant outcome is unresolved".into())
}

fn unavailable(error: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable(error.to_string())
}

#[cfg(test)]
#[path = "retained_transition_tests.rs"]
pub(crate) mod tests;
