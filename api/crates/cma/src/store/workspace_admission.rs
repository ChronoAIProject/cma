//! Transactional Mongo admission and removal for workspace aggregates.
use crate::store::transaction_terminal::GuardedTerminal as _;
mod commit;
pub(super) mod retry;
use super::sandbox_events::{append_in_transaction, reconcile_event};
use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, signed, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
};
use super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::work_claim_authority::without_ledger_maintenance;
use super::work_lease_update::replacement;
use super::workspace_removal::apply_removal;
use super::workspaces::{read_workspace, workspace_document};
use layer2_core::ids::{SubjectId, WorkspaceId};
use layer2_core::sandbox::store::{
    SandboxStoreError, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionResult,
    WorkspaceAdmissionStore,
};
use layer2_core::sandbox::{
    SandboxEvent, SandboxEventKind, SandboxReviveCause, Workspace, WorkspaceOrigin,
    WorkspaceStatus, WorkspaceWorkStep,
};
use mongodb::bson::{doc, Bson};
pub(super) use retry::ApplyError;
use retry::{exhausted, retry_delay};

pub(super) const TRANSACTION_ATTEMPTS: usize = 16;

enum AdmissionReceipt {
    Created {
        workspace: Workspace,
        event: SandboxEvent,
        revival_event: Option<Box<SandboxEvent>>,
    },
    Existing(Workspace),
}

impl AdmissionReceipt {
    fn result(&self) -> WorkspaceAdmissionResult {
        match self {
            Self::Created { workspace, .. } => WorkspaceAdmissionResult::Created(workspace.clone()),
            Self::Existing(workspace) => WorkspaceAdmissionResult::Existing(workspace.clone()),
        }
    }
}

#[async_trait::async_trait]
impl WorkspaceAdmissionStore for MongoSandboxStore {
    async fn admit_workspace(
        &self,
        admission: WorkspaceAdmission,
    ) -> Result<WorkspaceAdmissionResult, SandboxStoreError> {
        self.commit_workspace_admission(admission, false).await
    }

    async fn admit_catalogue_workspace(
        &self,
        admission: WorkspaceAdmission,
    ) -> Result<WorkspaceAdmissionResult, SandboxStoreError> {
        self.commit_workspace_admission(admission, true).await
    }

    async fn begin_workspace_prepare_recovery(
        &self,
        id: &WorkspaceId,
        caller: &layer2_core::Caller,
        operation_id: &str,
        now: String,
    ) -> Result<Workspace, SandboxStoreError> {
        super::workspace_prepare_recovery::begin(self, id, caller, operation_id, now).await
    }

    async fn begin_workspace_removal(
        &self,
        id: &WorkspaceId,
        owner: &SubjectId,
        now: String,
    ) -> Result<Workspace, SandboxStoreError> {
        let mut last_transient = None;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            retry_delay(attempt).await;
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    last_transient = Some(error.to_string());
                    continue;
                }
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
            let workspace = match apply_removal(self, id, owner, &now, &mut session).await {
                Ok(workspace) => workspace,
                Err(ApplyError::Domain(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(error);
                }
                Err(ApplyError::Retry(error)) | Err(ApplyError::Driver(error))
                    if is_transient(&error) =>
                {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Retry(error)) => {
                    let _ = session.abort_guarded().await;
                    last_transient = Some(error.to_string());
                    continue;
                }
                Err(ApplyError::Driver(error)) => {
                    let _ = session.abort_guarded().await;
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(workspace),
                CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
                CommitOutcome::Unknown(error) => {
                    return if reconcile_workspace(self, &workspace).await? {
                        Ok(workspace)
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "workspace removal admission commit remained unknown: {error}"
                        )))
                    };
                }
                CommitOutcome::Failed(error) => {
                    return Err(SandboxStoreError::Unavailable(error.to_string()));
                }
            }
        }
        Err(exhausted("workspace removal admission", last_transient))
    }
}

async fn apply_admission(
    store: &MongoSandboxStore,
    admission: &WorkspaceAdmission,
    catalogue: bool,
    session: &mut mongodb::ClientSession,
) -> Result<AdmissionReceipt, ApplyError> {
    store
        .bindings
        .admit_workspace(admission, session)
        .await
        .map_err(ApplyError::Domain)?;
    let candidate = &admission.workspace;
    lock_sandbox(store, candidate.sandbox_id.as_str(), session).await?;
    let sandbox = store
        .sandboxes
        .find_one(doc! { "_id": candidate.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ApplyError::Domain)?
        .ok_or(ApplyError::Domain(SandboxStoreError::SandboxNotReady))?;
    if !admission.kind.accepts(&sandbox) {
        return Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady));
    }
    candidate
        .validate_for_admission(&sandbox, sandbox.status)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    if !candidate.origin_matches_sandbox_profile(&sandbox) {
        return Err(ApplyError::Domain(
            SandboxStoreError::WorkspaceOriginConflict,
        ));
    }

    if store.workspaces.find_one(doc! {
        "owned_workspace_cleanup_admission_v1":{"$exists":true},
        "$and":[{"$or":[{"sandbox_id":candidate.sandbox_id.as_str()},{"record.sandbox_id":candidate.sandbox_id.as_str()}]},
            {"$or":[{"name":&candidate.name},{"record.name":&candidate.name},{"path":&candidate.path},{"record.path":&candidate.path}]}]
    }).session(&mut *session).await.map_err(ApplyError::Driver)?.is_some() {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNameConflict));
    }
    if admission.kind != WorkspaceAdmissionKind::Manual {
        let origin_filter = match &candidate.origin {
            WorkspaceOrigin::SandboxProfile { index } => doc! {
                "sandbox_id": candidate.sandbox_id.as_str(),
                "origin_type": "sandbox_profile",
                "origin_index": i64::from(*index),
            },
            WorkspaceOrigin::Trigger { launch_id } => doc! {
                "sandbox_id": candidate.sandbox_id.as_str(),
                "origin_type": "trigger",
                "origin_launch_id": launch_id,
            },
            WorkspaceOrigin::Lifecycle { operation_id } => doc! {
                "sandbox_id": candidate.sandbox_id.as_str(),
                "origin_type": "lifecycle",
                "origin_operation_id": operation_id,
            },
            WorkspaceOrigin::Manual => {
                return Err(ApplyError::Domain(
                    SandboxStoreError::WorkspaceOriginConflict,
                ));
            }
        };
        if let Some(document) = store
            .workspaces
            .find_one(crate::store::work_schema::query(origin_filter))
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
        {
            let existing = read_workspace(&document).map_err(ApplyError::Domain)?;
            return if same_profile_origin(&existing, candidate) {
                Ok(AdmissionReceipt::Existing(existing))
            } else {
                Err(ApplyError::Domain(
                    SandboxStoreError::WorkspaceOriginConflict,
                ))
            };
        }
    }
    if catalogue {
        let profile = &candidate.profile;
        let database = store.client.database(&store.workspaces.namespace().db);
        if matches!(&candidate.origin, WorkspaceOrigin::Trigger { .. })
            || !super::profile_pin_admission::workspace(
                &database,
                profile.id.as_str(),
                profile.revision,
                Some(&profile.digest),
                session,
            )
            .await
            .map_err(ApplyError::Driver)?
        {
            super::profile_pin_admission::frozen::workspace(
                &database, candidate, &sandbox, session,
            )
            .await
            .map_err(ApplyError::Driver)?
            .require()
            .map_err(ApplyError::Domain)?;
        }
    }
    if store
        .workspaces
        .find_one(doc! {
            "sandbox_id": candidate.sandbox_id.as_str(),
            "name": &candidate.name,
            "removed_at": Bson::Null,
        })
        .projection(doc! { "_id": 1 })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .is_some()
    {
        return Err(ApplyError::Domain(SandboxStoreError::WorkspaceNameConflict));
    }
    let live = super::shared_child_capacity::workspaces(store, &sandbox.id, session)
        .await
        .map_err(|error| match error {
            super::shared_child_capacity::CapacityError::Domain(error) => ApplyError::Domain(error),
            super::shared_child_capacity::CapacityError::Driver(error) => ApplyError::Driver(error),
        })?;
    if live >= u64::from(sandbox.limits.max_workspaces) {
        return Err(ApplyError::Domain(
            SandboxStoreError::WorkspaceLimitReached {
                limit: sandbox.limits.max_workspaces,
                live: u8::try_from(live).unwrap_or(u8::MAX),
            },
        ));
    }
    super::budget::require_open(
        store,
        super::budget::now_seconds(),
        admission.admission_cause(),
        session,
    )
    .await?;
    let prepared =
        match super::native_prepared::adopt_workspace(store, &sandbox, candidate, session)
            .await
            .map_err(ApplyError::Domain)?
        {
            Some(prepared) => prepared,
            None => super::warm_sandboxes::adopt_workspace(store, &sandbox, candidate, session)
                .await
                .map_err(ApplyError::Domain)?,
        };
    let candidate = &prepared;
    let revival_event = if admission.kind == WorkspaceAdmissionKind::SandboxProfile {
        None
    } else {
        let mut next = sandbox.clone();
        let cause = match admission.kind {
            WorkspaceAdmissionKind::Trigger => SandboxReviveCause::TriggerLaunch,
            _ => SandboxReviveCause::WorkspaceCreation,
        };
        let destination = store
            .managed_resume_destination(&sandbox, session)
            .await
            .map_err(ApplyError::Domain)?;
        let revival = next
            .record_creation_admission_with_managed_runtime(
                cause,
                destination.as_deref(),
                admission.event_at.clone(),
            )
            .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
        let updated = store
            .sandboxes
            .update_one(
                crate::store::work_schema::query(doc! {
                    "_id": sandbox.id.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "channel_reuse_cleanup_execution": { "$exists": false },
                    "revision": signed(sandbox.revision, "sandbox revision")
                        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error)))?,
                }),
                replacement(sandbox_replacement_fields(&next).map_err(ApplyError::Domain)?),
            )
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?;
        if updated.matched_count != 1 {
            return Err(ApplyError::Domain(SandboxStoreError::Conflict));
        }
        crate::store::sandbox_usage::save::record_saved(store, session, &next, sandbox.revision)
            .await?;
        match revival {
            Some(event) => Some(Box::new(
                append_in_transaction(
                    store,
                    session,
                    &sandbox.id,
                    admission.event_at.clone(),
                    event,
                )
                .await?,
            )),
            None => None,
        }
    };
    store
        .workspaces
        .insert_one(workspace_document(candidate).map_err(ApplyError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_insert)?;
    let event = append_in_transaction(
        store,
        session,
        &candidate.sandbox_id,
        admission.event_at.clone(),
        SandboxEventKind::WorkspaceCreated {
            workspace_id: candidate.id.clone(),
        },
    )
    .await?;
    Ok(AdmissionReceipt::Created {
        workspace: candidate.clone(),
        event,
        revival_event,
    })
}

pub(super) async fn lock_sandbox(
    store: &MongoSandboxStore,
    sandbox_id: &str,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    let locked = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(without_ledger_maintenance(doc! {
                "_id": sandbox_id,
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "channel_reuse_cleanup_execution": { "$exists": false },
                super::native_retirement_action::FIELD: { "$exists": false },
            })),
            doc! { "$inc": { "workspace_admission_fence": 1_i64 } },
        )
        .session(session)
        .await
        .map_err(ApplyError::Driver)?;
    if locked.matched_count == 1 {
        Ok(())
    } else {
        Err(ApplyError::Domain(SandboxStoreError::SandboxNotReady))
    }
}

fn validate_fresh(
    workspace: &Workspace,
    kind: WorkspaceAdmissionKind,
) -> Result<(), SandboxStoreError> {
    let origin_matches = matches!(
        (&workspace.origin, kind),
        (WorkspaceOrigin::Manual, WorkspaceAdmissionKind::Manual)
            | (
                WorkspaceOrigin::Lifecycle { .. },
                WorkspaceAdmissionKind::Lifecycle
            )
            | (
                WorkspaceOrigin::SandboxProfile { .. },
                WorkspaceAdmissionKind::SandboxProfile
            )
            | (
                WorkspaceOrigin::Trigger { .. },
                WorkspaceAdmissionKind::Trigger
            )
    );
    if !origin_matches
        || workspace.revision != 1
        || workspace.status != WorkspaceStatus::Preparing
        || workspace.work.step != WorkspaceWorkStep::Prepare
        || workspace.resolved_commit.is_some()
        || workspace.ready_at.is_some()
        || workspace.failed.is_some()
        || workspace.removed_at.is_some()
    {
        return Err(SandboxStoreError::InvalidTransaction(
            "workspace admission is not a fresh preparing aggregate",
        ));
    }
    Ok(())
}

fn same_profile_origin(existing: &Workspace, candidate: &Workspace) -> bool {
    existing.owner == candidate.owner
        && existing.profile.id == candidate.profile.id
        && existing.profile.revision == candidate.profile.revision
        && existing.profile.digest == candidate.profile.digest
}

fn classify_insert(error: mongodb::error::Error) -> ApplyError {
    if is_duplicate_key(&error) {
        ApplyError::Domain(SandboxStoreError::Duplicate)
    } else if is_transient(&error) {
        ApplyError::Retry(error)
    } else {
        ApplyError::Driver(error)
    }
}

async fn reconcile_admission(
    store: &MongoSandboxStore,
    receipt: &AdmissionReceipt,
) -> Result<bool, SandboxStoreError> {
    match receipt {
        AdmissionReceipt::Created {
            workspace,
            event,
            revival_event,
        } => {
            let revival_matches = match revival_event {
                Some(event) => reconcile_event(store, event).await?.is_some(),
                None => true,
            };
            Ok(reconcile_workspace(store, workspace).await?
                && reconcile_event(store, event).await?.is_some()
                && revival_matches)
        }
        AdmissionReceipt::Existing(workspace) => reconcile_workspace(store, workspace).await,
    }
}

pub(super) async fn reconcile_workspace(
    store: &MongoSandboxStore,
    expected: &Workspace,
) -> Result<bool, SandboxStoreError> {
    let stored = store
        .workspaces
        .find_one(doc! { "_id": expected.id.as_str() })
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_workspace)
        .transpose()?;
    Ok(stored.as_ref() == Some(expected))
}
