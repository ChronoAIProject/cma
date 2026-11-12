use layer2_core::ids::SandboxId;
use layer2_core::sandbox::store::{
    apply_image_rotation, apply_inactivity_pause, SandboxImageRotationRequest,
    SandboxInactivityPauseRequest, SandboxMaintenanceApplication, SandboxMaintenanceOutcome,
    SandboxStoreError,
};
use layer2_core::sandbox::{Sandbox, SandboxEvent};
use mongodb::bson::{doc, Bson};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::super::agent_log_integrity::TransactionLogError;
use super::super::lifecycle_execution::{load_sandbox_agents, LifecycleLoadError};
use super::super::sandbox_events::{
    append_in_transaction, reconcile_event_with_snapshot_session, validate_in_transaction,
    TransactionEventError,
};
use super::super::sandbox_mongo::{signed, unsigned, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::evidence::{fence_live_agents, live_activities, open_session};

pub(super) enum MaintenanceRequest {
    Inactivity(SandboxInactivityPauseRequest),
    Rotation(SandboxImageRotationRequest),
}

impl MaintenanceRequest {
    fn sandbox_id(&self) -> &SandboxId {
        match self {
            Self::Inactivity(request) => &request.sandbox_id,
            Self::Rotation(request) => &request.sandbox_id,
        }
    }

    fn requested_at(&self) -> &str {
        match self {
            Self::Inactivity(request) => &request.requested_at,
            Self::Rotation(request) => &request.requested_at,
        }
    }
}

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

pub(super) async fn apply(
    store: &MongoSandboxStore,
    request: &MaintenanceRequest,
    session: &mut ClientSession,
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
    let agents = load_sandbox_agents(store, &current.id, session).await?;
    let application = match request {
        MaintenanceRequest::Inactivity(request) => {
            let activities =
                live_activities(store, &agents, session, request.requested_at.as_str()).await?;
            if platform_stop
                && activities.is_empty()
                && open_session(store, &current, &request.requested_at, session).await?
            {
                return Ok(MaintenanceReceipt {
                    sandbox: current,
                    event: None,
                    outcome: SandboxMaintenanceOutcome::NotEligible,
                });
            }
            apply_inactivity_pause(&current, &activities, request)
        }
        MaintenanceRequest::Rotation(request) => apply_image_rotation(&current, &agents, request),
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
    validate_in_transaction(store, &current.id, last_seq, session).await?;
    // Snapshot isolation needs actual writes to prevent a concurrent Agent turn
    // from committing after the evidence read without conflicting with admission.
    if matches!(request, MaintenanceRequest::Rotation(_)) {
        if !super::evidence::fence_idle_workspaces(store, &current, session).await? {
            return Ok(MaintenanceReceipt {
                sandbox: current,
                event: None,
                outcome: SandboxMaintenanceOutcome::NotEligible,
            });
        }
        super::evidence::fence_agents(store, &agents, session).await?;
    } else {
        // Pending or leased Workspace work is never interrupted by a platform
        // stop, with or without live Agents.
        if platform_stop
            && !super::evidence::fence_idle_workspaces(store, &current, session).await?
        {
            return Ok(MaintenanceReceipt {
                sandbox: current,
                event: None,
                outcome: SandboxMaintenanceOutcome::NotEligible,
            });
        }
        fence_live_agents(store, &agents, session).await?;
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
    let advanced = store
        .sandboxes
        .update_one(
            revision_filter,
            doc! { "$set": sandbox_replacement_fields(&sandbox).map_err(ApplyError::Domain)? },
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
        // On activation, historical profile availability is intentionally
        // ignored. An explicit per-Sandbox preference is the compatibility
        // boundary; this makes existing lifecycle Sandboxes eligible without
        // mutating their pinned profile.
        return fence_policy(store, sandbox, session).await;
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

pub(super) async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &MaintenanceReceipt,
    snapshot_time: mongodb::bson::Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let actual = store
        .sandboxes
        .find_one(doc! { "_id": receipt.sandbox.id.as_str() })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?
        .as_ref()
        .map(read_sandbox)
        .transpose()?;
    if actual.as_ref() != Some(&receipt.sandbox) {
        return Ok(false);
    }
    match &receipt.event {
        Some(event) => Ok(
            reconcile_event_with_snapshot_session(store, event, &mut session)
                .await?
                .is_some(),
        ),
        None => Ok(false),
    }
}
