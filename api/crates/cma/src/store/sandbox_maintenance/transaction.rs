use layer2_core::sandbox::store::{
    apply_image_rotation, apply_proved_inactivity_pause, apply_retained_runtime_abort,
    apply_retained_runtime_transition, SandboxMaintenanceApplication, SandboxMaintenanceOutcome,
    SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxEvent};
use mongodb::bson::{doc, Bson};
use mongodb::ClientSession;

use super::super::agent_log_integrity::{IntegrityFailurePolicy, TransactionLogError};
use super::super::lifecycle_execution::{load_sandbox_agents, LifecycleLoadError};
use super::super::sandbox_events::{
    append_in_transaction, validate_in_transaction, TransactionEventError,
};
use super::super::sandbox_mongo::{signed, unsigned, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::evidence::{
    fence_live_agents, live_activities_with_sources, open_session, scoped::CapturedActivities,
    LogValidation,
};

mod reconciliation;
mod request;
mod scoped;
pub(super) use reconciliation::reconcile;
pub(super) use request::MaintenanceRequest;
pub(super) use scoped::attempt;

pub(super) struct MaintenanceReceipt {
    pub(super) sandbox: Sandbox,
    pub(super) event: Option<SandboxEvent>,
    pub(super) outcome: SandboxMaintenanceOutcome,
}

#[derive(Debug)]
pub(super) enum ApplyError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

impl From<LifecycleLoadError> for ApplyError {
    fn from(error: LifecycleLoadError) -> Self {
        match error {
            LifecycleLoadError::Domain(error) => Self::Domain(error),
            LifecycleLoadError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionEventError> for ApplyError {
    fn from(error: TransactionEventError) -> Self {
        match error {
            TransactionEventError::Domain(error) => Self::Domain(error),
            TransactionEventError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<TransactionLogError> for ApplyError {
    fn from(error: TransactionLogError) -> Self {
        match error {
            TransactionLogError::Domain(error) => Self::Domain(error),
            TransactionLogError::Driver(error) => Self::Driver(error),
        }
    }
}

#[cfg(test)]
pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &MaintenanceRequest,
    session: &mut ClientSession,
) -> Result<MaintenanceReceipt, ApplyError> {
    apply_with_sources(
        store,
        request,
        session,
        None,
        IntegrityFailurePolicy::Quarantine,
    )
    .await
}

pub(super) async fn apply_with_sources(
    store: &MongoSandboxStore,
    request: &MaintenanceRequest,
    session: &mut ClientSession,
    sources: Option<&CapturedActivities>,
    failure_policy: IntegrityFailurePolicy,
) -> Result<MaintenanceReceipt, ApplyError> {
    let document = store
        .sandboxes
        .find_one(doc! { "_id": request.sandbox_id().as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
        .ok_or(ApplyError::Domain(SandboxStoreError::NotFound))?;
    let current = read_sandbox(&document).map_err(ApplyError::Domain)?;
    // A platform stop overrides every keep-alive preference without reading
    // or changing it (decision 26); an inactivity pause honours them.
    let platform_stop = matches!(
        request,
        MaintenanceRequest::Inactivity(request) if !request.cause.honours_keep_alive()
    );
    if matches!(request, MaintenanceRequest::Inactivity(_))
        && !platform_stop
        && keep_alive_and_fence_policy(
            store,
            &current,
            matches!(request, MaintenanceRequest::Inactivity(request) if request.auto_pause_enabled),
            session,
        )
        .await?
    {
        return Ok(MaintenanceReceipt {
            sandbox: current,
            event: None,
            outcome: SandboxMaintenanceOutcome::NotEligible,
        });
    }
    let last_seq = document
        .get_i64("last_seq")
        .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
        .and_then(|seq| unsigned(seq, "sandbox event sequence").map_err(SandboxStoreError::Corrupt))
        .map_err(ApplyError::Domain)?;
    let desktop_activity_fence = match document.get("desktop_activity_fence") {
        None => 0,
        Some(Bson::Int64(value)) => unsigned(*value, "desktop activity fence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
        Some(Bson::Int32(value)) => unsigned(i64::from(*value), "desktop activity fence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
        Some(_) => {
            return Err(ApplyError::Domain(SandboxStoreError::Corrupt(
                "desktop activity fence is not an integer".into(),
            )))
        }
    };
    if let MaintenanceRequest::Custom(admission) = request {
        if admission.is_replay(&current).map_err(ApplyError::Domain)? {
            return Ok(MaintenanceReceipt {
                sandbox: current,
                event: None,
                outcome: SandboxMaintenanceOutcome::AlreadyScheduled,
            });
        }
    }
    let agents = load_sandbox_agents(store, &current.id, session).await?;
    if let Some(basis) = request.basis() {
        super::custom_runtime::validate_admission(
            store, &document, &current, &agents, basis, session,
        )
        .await?;
    }
    let application = match request {
        MaintenanceRequest::Inactivity(request) => {
            let bounded_legacy_log =
                store.bounded_inactivity_legacy_log && request.auto_pause_enabled && !platform_stop;
            if super::evidence::queued_control(store, &current.id, session).await?
                || (bounded_legacy_log
                    && !super::evidence::legacy_log_budget::within(
                        store, &agents, sources, session,
                    )
                    .await?)
            {
                return Ok(MaintenanceReceipt {
                    sandbox: current,
                    event: None,
                    outcome: SandboxMaintenanceOutcome::NotEligible,
                });
            }
            let strict_sessions = store.strict_inactivity_session_evidence
                && request.auto_pause_enabled
                && !platform_stop;
            if strict_sessions
                && !super::evidence::sessions::clear(
                    store,
                    &current.id,
                    &request.requested_at,
                    session,
                )
                .await?
            {
                return Ok(MaintenanceReceipt {
                    sandbox: current,
                    event: None,
                    outcome: SandboxMaintenanceOutcome::NotEligible,
                });
            }
            let activities = live_activities_with_sources(
                store,
                &agents,
                session,
                request.requested_at.as_str(),
                sources,
                LogValidation {
                    failure_policy,
                    legacy_formats_checked: bounded_legacy_log,
                },
                strict_sessions,
            )
            .await?;
            if !strict_sessions
                && activities.is_empty()
                && open_session(store, &current, &request.requested_at, session).await?
            {
                return Ok(MaintenanceReceipt {
                    sandbox: current,
                    event: None,
                    outcome: SandboxMaintenanceOutcome::NotEligible,
                });
            }
            let Some(workspaces) =
                super::evidence::idle_workspaces(store, &current, session).await?
            else {
                return Ok(MaintenanceReceipt {
                    sandbox: current,
                    event: None,
                    outcome: SandboxMaintenanceOutcome::NotEligible,
                });
            };
            let agentless_idle = activities.is_empty()
                && layer2_core::sandbox::agentless_idle_with_children(
                    &current,
                    &agents,
                    &workspaces,
                    &request.requested_at,
                    request.pause_after_seconds,
                )
                && super::evidence::agent_commands_clear(store, &agents, session).await?;
            apply_proved_inactivity_pause(&current, &activities, request, agentless_idle)
        }
        MaintenanceRequest::Rotation(request) => apply_image_rotation(&current, &agents, request),
        MaintenanceRequest::FleetRotation(request, basis) => {
            layer2_core::sandbox::store::apply_scoped_image_rotation(
                &current, &agents, request, basis,
            )
        }
        MaintenanceRequest::Custom(admission) => {
            layer2_core::sandbox::store::apply_custom_runtime_migration(
                &current, &agents, admission,
            )
        }
        MaintenanceRequest::RetainedTransition {
            request,
            requested_at,
        } => {
            if super::evidence::queued_control(store, &current.id, session).await?
                || !super::evidence::agent_commands_clear(store, &agents, session).await?
                || open_session(store, &current, requested_at, session).await?
            {
                return Ok(MaintenanceReceipt {
                    sandbox: current,
                    event: None,
                    outcome: SandboxMaintenanceOutcome::NotEligible,
                });
            }
            apply_retained_runtime_transition(&current, &agents, request, requested_at)
        }
        MaintenanceRequest::RetainedTransitionAbort {
            receipt,
            requested_at,
        } => apply_retained_runtime_abort(&current, receipt, requested_at),
    }
    .map_err(ApplyError::Domain)?;
    let (expected_revision, sandbox, event) = match application {
        SandboxMaintenanceApplication::Complete(outcome) => {
            return Ok(MaintenanceReceipt {
                sandbox: current,
                event: None,
                outcome,
            });
        }
        SandboxMaintenanceApplication::Persist {
            expected_revision,
            sandbox,
            event,
        } => (expected_revision, *sandbox, event),
    };
    if store
        .legacy_reuse_blocks_quiescence(&current, &sandbox, session)
        .await
        .map_err(ApplyError::Driver)?
    {
        return Ok(MaintenanceReceipt {
            sandbox: current,
            event: None,
            outcome: SandboxMaintenanceOutcome::NotEligible,
        });
    }
    // The sweep's cached answer only proposes a pause. A lift or policy
    // change must conflict with the durable pause admission itself.
    if platform_stop
        && !super::super::budget::fence_stop(store, session)
            .await
            .map_err(|error| match error {
                super::super::agent_conversation_store::ApplyError::Domain(error) => {
                    ApplyError::Domain(error)
                }
                super::super::agent_conversation_store::ApplyError::Driver(error) => {
                    ApplyError::Driver(error)
                }
            })?
    {
        return Ok(MaintenanceReceipt {
            sandbox: current,
            event: None,
            outcome: SandboxMaintenanceOutcome::NotEligible,
        });
    }
    // Acquire the global settings write only for an eligible candidate, after
    // evidence reads. Snapshot isolation still conflicts with a policy change;
    // the shorter write hold avoids serializing unrelated discovery work.
    if let MaintenanceRequest::Inactivity(pause) = request {
        if pause.auto_pause_enabled
            && !platform_stop
            && !super::policy_fence::fence(store, pause, session).await?
        {
            return Ok(MaintenanceReceipt {
                sandbox: current,
                event: None,
                outcome: SandboxMaintenanceOutcome::NotEligible,
            });
        }
    }
    validate_in_transaction(store, &current.id, last_seq, session).await?;
    // Snapshot isolation needs actual writes to prevent a concurrent Agent turn
    // from committing after the evidence read without conflicting with admission.
    if matches!(
        request,
        MaintenanceRequest::Rotation(_)
            | MaintenanceRequest::FleetRotation(..)
            | MaintenanceRequest::Custom(_)
            | MaintenanceRequest::RetainedTransition { .. }
    ) {
        if !super::evidence::fence_idle_workspaces(store, &current, session).await? {
            return Ok(MaintenanceReceipt {
                sandbox: current,
                event: None,
                outcome: SandboxMaintenanceOutcome::NotEligible,
            });
        }
        super::evidence::fence_agents(store, &agents, session).await?;
    } else {
        // Workspace evidence was fenced before planning. Terminal Agent rows
        // also fence command admission when the complete live set is empty.
        if agents.iter().any(|agent| agent.status.is_live()) {
            fence_live_agents(store, &agents, session).await?;
        } else {
            super::evidence::fence_agents(store, &agents, session).await?;
        }
    }
    let mut revision_filter = doc! {
        "_id": current.id.as_str(),
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "channel_reuse_cleanup_execution": { "$exists": false },
        "revision": signed(expected_revision, "sandbox maintenance revision")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        "last_seq": signed(last_seq, "sandbox maintenance event sequence")
            .map_err(|_| ApplyError::Domain(SandboxStoreError::SequenceConflict))?,
    };
    if matches!(
        request,
        MaintenanceRequest::RetainedTransition { .. }
            | MaintenanceRequest::RetainedTransitionAbort { .. }
    ) {
        revision_filter =
            crate::store::work_claim_authority::without_ledger_maintenance(revision_filter);
    }
    if desktop_activity_fence == 0 {
        revision_filter.insert(
            "$or",
            vec![
                Bson::Document(doc! { "desktop_activity_fence": 0_i64 }),
                Bson::Document(doc! { "desktop_activity_fence": { "$exists": false } }),
            ],
        );
    } else {
        revision_filter.insert(
            "desktop_activity_fence",
            signed(desktop_activity_fence, "desktop activity fence")
                .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
        );
    }
    if sandbox.lifecycle.custom_runtime.is_some()
        || sandbox.lifecycle.runtime_migration_admission.is_some()
    {
        crate::store::work_schema::floor::retain_runtime_migration(store, session)
            .await
            .map_err(ApplyError::Driver)?;
    }
    let advanced = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(revision_filter),
            crate::store::work_lease_update::replacement(
                sandbox_replacement_fields(&sandbox).map_err(ApplyError::Domain)?,
            ),
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if advanced.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::sandbox_usage::save::record_saved(store, session, &sandbox, expected_revision)
        .await?;
    let event = append_in_transaction(
        store,
        session,
        &current.id,
        request.requested_at().to_owned(),
        event,
    )
    .await?;
    Ok(MaintenanceReceipt {
        sandbox,
        event: Some(event),
        outcome: SandboxMaintenanceOutcome::Scheduled,
    })
}

async fn keep_alive_and_fence_policy(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    auto_pause_enabled: bool,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    if sandbox.profile.always_on == Some(true) {
        return Ok(true);
    }
    if auto_pause_enabled {
        // Global policy ignores historical Profile availability. Its settings
        // revision and the explicit preference on the Sandbox are fenced by this
        // transaction, including when a retained Profile has been removed.
        return Ok(false);
    }
    if store
        .bindings
        .keep_alive(sandbox, session)
        .await
        .map_err(ApplyError::Domain)?
        == Some(true)
    {
        return Ok(true);
    }
    let Some(document) = store
        .sandbox_profiles
        .find_one(doc! { "_id": sandbox.profile.id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?
    else {
        return super::retained_availability::keep_alive(store, sandbox, session).await;
    };
    let profile = super::super::sandbox_profiles::read_profile(&document)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    if profile.availability.always_keep_alive {
        return Ok(true);
    }
    fence_policy(store, sandbox, session).await
}

async fn fence_policy(
    store: &MongoSandboxStore,
    sandbox: &Sandbox,
    session: &mut ClientSession,
) -> Result<bool, ApplyError> {
    // An actual write conflicts with any concurrent policy edit. Merely reading
    // the Profile under snapshot isolation permits an obsolete pause to commit.
    let fenced = store
        .sandbox_profiles
        .update_one(
            doc! { "_id": sandbox.profile.id.as_str() },
            doc! { "$inc": { "inactivity_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    if fenced.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    Ok(false)
}
