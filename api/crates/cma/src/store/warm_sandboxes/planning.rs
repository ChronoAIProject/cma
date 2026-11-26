use crate::store::transaction_terminal::GuardedTerminal as _;
use futures_util::TryStreamExt;
use layer2_core::profiles::{ProfileVisibility, SandboxProfileSnapshot};
use layer2_core::sandbox::{
    ProvisioningExecution, SandboxStoreError, WarmSandbox, WarmSandboxPlan, WarmSandboxState,
};
use layer2_core::{SandboxProfileId, ServiceId};
use mongodb::bson::{doc, Bson};

use super::{document, read, unavailable};
use crate::store::sandbox_mongo::{
    commit_transaction, is_transient, start_transaction, CommitOutcome, MongoSandboxStore,
};

enum PlanError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}
impl From<SandboxStoreError> for PlanError {
    fn from(error: SandboxStoreError) -> Self {
        Self::Domain(error)
    }
}
impl From<mongodb::error::Error> for PlanError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}

impl MongoSandboxStore {
    pub(crate) async fn reconcile_warmup_target(
        &self,
        profile_id: &SandboxProfileId,
        runtime_version: &str,
        baseline: &[ServiceId],
        execution: ProvisioningExecution,
        at: &str,
    ) -> Result<(), SandboxStoreError> {
        for _ in 0..16 {
            let mut session = self.client.start_session().await.map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            match plan(
                self,
                profile_id,
                runtime_version,
                baseline,
                execution,
                at,
                &mut session,
            )
            .await
            {
                Ok(()) => (),
                Err(PlanError::Driver(error)) if is_transient(&error) => {
                    let _ = session.abort_guarded().await;
                    tokio::task::yield_now().await;
                    continue;
                }
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    return Err(match error {
                        PlanError::Domain(error) => error,
                        PlanError::Driver(error) => unavailable(error),
                    });
                }
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(()),
                CommitOutcome::Retry(_) => continue,
                // No side effect occurs until a later durable read. Reconciliation
                // can safely recompute the bounded target after an ambiguous commit.
                CommitOutcome::Unknown(error) | CommitOutcome::Failed(error) => {
                    return Err(unavailable(error))
                }
            }
        }
        Err(SandboxStoreError::Conflict)
    }
}

async fn plan(
    store: &MongoSandboxStore,
    profile_id: &SandboxProfileId,
    runtime_version: &str,
    baseline: &[ServiceId],
    execution: ProvisioningExecution,
    at: &str,
    session: &mut mongodb::ClientSession,
) -> Result<(), PlanError> {
    let profile_doc = store
        .sandbox_profiles
        .find_one(doc! { "_id": profile_id.as_str() })
        .session(&mut *session)
        .await?;
    let mut desired = None;
    let mut target = 0;
    if let Some(document) = profile_doc {
        let profile = crate::store::sandbox_profiles::read_profile(&document)
            .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
        // Serialize counts, allocation and admissions with actual Profile writes.
        store
            .sandbox_profiles
            .update_one(
                doc! { "_id": profile_id.as_str() },
                doc! { "$inc": { "warmup_fence": 1_i64 } },
            )
            .session(&mut *session)
            .await?;
        if profile.visibility == ProfileVisibility::Public
            && profile.retired_at.is_none()
            && profile.availability.warmup_enabled
        {
            let revision = document
                .get_i64("last_revision")
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            let revision_doc = store
                .sandbox_profile_revisions
                .find_one(doc! {
                    "profile_id": profile_id.as_str(), "revision": revision,
                })
                .session(&mut *session)
                .await?
                .ok_or_else(|| {
                    SandboxStoreError::Corrupt("warmup published revision missing".into())
                })?;
            let revision = crate::store::sandbox_profiles::read_revision(&revision_doc)
                .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))?;
            let reaches = revision.content.credential_services(baseline);
            let plan = WarmSandboxPlan {
                workspaces: super::hierarchy::resolve(
                    store,
                    &revision.content.workspace_profiles,
                    session,
                )
                .await?,
                profile: SandboxProfileSnapshot::from(&revision),
                reaches,
                runtime_version: runtime_version.into(),
                execution,
            };
            plan.validate()?;
            desired = Some(plan);
            target = usize::from(profile.availability.warmup_count);
        }
    }
    let mut cursor = store
        .warm_sandboxes
        .find(doc! {
            "profile_id": profile_id.as_str(),
            "state": { "$in": ["preparing", "installing", "ready", "retiring"] },
        })
        .sort(doc! { "_id": 1 })
        .session(&mut *session)
        .await?;
    let records = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
    // Failed/stale runtimes still occupy capacity until their exact teardown
    // completes. An outage must not create an unbounded replacement backlog.
    let occupied = records.len();
    let mut retained = 0;
    for old in records {
        let mut record = read(&old)?;
        if record.state == WarmSandboxState::Retiring {
            continue;
        }
        let home_matches = record.home.as_ref().map(|home| &home.request.policy)
            == store.warm_home_policy.as_ref()
            || (record.state == WarmSandboxState::Preparing && record.home.is_none());
        if Some(&record.plan) == desired.as_ref() && home_matches && retained < target {
            retained += 1;
        } else {
            record.state = WarmSandboxState::Retiring;
            record.revision = record
                .revision
                .checked_add(1)
                .ok_or(SandboxStoreError::Conflict)?;
            record.updated_at = at.into();
            let mut next = document(&record)?;
            // Preserve the live worker lease: teardown waits until outstanding
            // preparation exits or its durable lease expires.
            for key in ["lease_token", "lease_until"] {
                next.insert(key, old.get(key).cloned().unwrap_or(Bson::Null));
            }
            next.insert("check_after", at);
            store
                .warm_sandboxes
                .replace_one(doc! { "_id": record.id.as_str() }, next)
                .session(&mut *session)
                .await?;
        }
    }
    if let Some(plan) = desired {
        for _ in 0..target
            .saturating_sub(retained)
            .min(target.saturating_sub(occupied))
        {
            let record = WarmSandbox::new(plan.clone(), at.into())?;
            let mut value = document(&record)?;
            value.insert("lease_token", Bson::Null);
            value.insert("lease_until", at);
            value.insert("check_after", at);
            store
                .warm_sandboxes
                .insert_one(value)
                .session(&mut *session)
                .await?;
        }
    }
    Ok(())
}
