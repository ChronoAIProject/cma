//! Commit one fenced Sandbox state transition with its ledger event.
use super::*;
use layer2_core::sandbox::{agent_io::CommandRevivalAuthorization, Sandbox};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    session: &mut mongodb::ClientSession,
    revival: (
        Option<&CommandRevivalAuthorization>,
        Option<&layer3_adapter::ReleasedRuntime>,
    ),
    expected_revision: u64,
    sandbox: &Sandbox,
    event_at: &str,
    event: &SandboxEventKind,
) -> Result<Receipt, ApplyError> {
    let next = expected_revision
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::Conflict))?;
    if sandbox.revision != next {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let aggregate = store
        .sandboxes
        .find_one(crate::store::work_schema::query(doc! {
            "_id": sandbox.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        }))
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let Some(aggregate) = aggregate else {
        let exists = store
            .sandboxes
            .find_one(doc! { "_id": sandbox.id.as_str() })
            .projection(doc! { "_id": 1 })
            .session(&mut *session)
            .await
            .map_err(ApplyError::Driver)?
            .is_some();
        return Err(ApplyError::Domain(if exists {
            SandboxStoreError::Conflict
        } else {
            SandboxStoreError::NotFound
        }));
    };
    let current = read_sandbox(&aggregate).map_err(ApplyError::Domain)?;
    if current.revision != expected_revision {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::native_retirement_action::preserve_custody(&aggregate, &current, sandbox)
        .map_err(ApplyError::Domain)?;
    current
        .validate_retirement_archive_successor(sandbox)
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    channel_reuse::require(store, &current, sandbox, session).await?;
    if revival.0.is_none()
        && current.has_managed_runtime_image()
        && current.runtime_update_request() != sandbox.runtime_update_request()
    {
        if let Some(release) = revival.1 {
            store
                .fence_released_admission(
                    release,
                    layer2_core::released_runtime::ReleasedLane::Agent,
                    session,
                )
                .await
                .map_err(ApplyError::Domain)?;
        }
    }
    let revival_receipt =
        command_revival::fence(store, session, revival, sandbox, event_at, event).await?;
    current
        .validate_service_access_transaction(sandbox, event, event_at)
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    current
        .validate_unpublished_recovery_save(sandbox)
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    current
        .validate_checkpoint_capture_successor(sandbox)
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    layer2_core::sandbox::ProvisioningExecution::validate_successor(
        current.work.execution,
        sandbox.work.execution,
    )
    .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    current
        .validate_runtime_generation_successor(sandbox)
        .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?;
    let previous = aggregate
        .get_i64("last_seq")
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))
        .and_then(|value| {
            unsigned(value, "sandbox event sequence")
                .map_err(SandboxStoreError::Corrupt)
                .map_err(ApplyError::Domain)
        })?;
    let seq = previous
        .checked_add(1)
        .ok_or(ApplyError::Domain(SandboxStoreError::SequenceConflict))?;
    let mut fields = sandbox_replacement_fields(sandbox).map_err(ApplyError::Domain)?;
    let (origin_type, origin_launch_id, origin_operation_id) =
        sandbox_origin_fields(sandbox).map_err(ApplyError::Domain)?;
    fields.insert(
        "last_seq",
        signed(seq, "sandbox event sequence")
            .map_err(SandboxStoreError::Corrupt)
            .map_err(ApplyError::Domain)?,
    );
    let advanced = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(
                crate::store::work_publication::sandbox(
                    doc! {
                        "_id": sandbox.id.as_str(),
                        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                        "channel_reuse_cleanup_execution": { "$exists": false },
                        "revision": signed(expected_revision, "expected sandbox revision")
                            .map_err(|_| ApplyError::Domain(SandboxStoreError::Conflict))?,
                        "last_seq": signed(previous, "sandbox event sequence")
                            .map_err(SandboxStoreError::Corrupt)
                            .map_err(ApplyError::Domain)?,
                        "origin_type": origin_type, "origin_launch_id": origin_launch_id,
                        "origin_operation_id": origin_operation_id,
                    },
                    sandbox.id.as_str(),
                )
                .map_err(ApplyError::Domain)?,
            ),
            crate::store::work_lease_update::replacement(fields),
        )
        .session(&mut *session)
        .await
        .map_err(classify_write)?;
    if advanced.matched_count != 1 {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    let event = SandboxEvent {
        sandbox_id: sandbox.id.clone(),
        seq,
        appended_at: event_at.to_owned(),
        kind: event.clone(),
    };
    store
        .sandbox_events
        .insert_one(event_document(&event).map_err(ApplyError::Domain)?)
        .session(&mut *session)
        .await
        .map_err(classify_sequence_write)?;
    crate::store::sandbox_usage::lifecycle_event(store, session, &event, Some(expected_revision))
        .await?;
    Ok(Receipt::SaveSandbox(event, revival_receipt))
}
