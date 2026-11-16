//! Capture and publication share one transaction, including checkpoint metadata.
use super::*;
use crate::store::{
    agent_capacity_recovery::source::sandbox_uses_segments,
    agent_log_segments::transaction_scope::TransactionScope,
};

pub(super) async fn attempt(
    store: &MongoSandboxStore,
    mutation: &SandboxCheckpointMutation,
    session: &mut mongodb::ClientSession,
) -> Result<PublicationReceipt, PublishError> {
    if !sandbox_uses_segments(store, &mutation.guard.sandbox_id).await? {
        start_transaction(session)
            .await
            .map_err(PublishError::Driver)?;
        return apply_publication(store, mutation, session).await;
    }
    let mut scope = TransactionScope::start(session)
        .await
        .map_err(PublishError::Driver)?;
    scope
        .fence_ordinary_work(store)
        .await
        .map_err(|error| match error {
            crate::store::work_publication::FenceError::Domain(error) => {
                PublishError::Domain(error)
            }
            crate::store::work_publication::FenceError::Driver(error) => {
                PublishError::Driver(error)
            }
        })?;
    let row = scope
        .find_one(
            &store.sandboxes,
            doc! {"_id": mutation.guard.sandbox_id.as_str()},
        )
        .await
        .map_err(PublishError::Driver)?
        .ok_or(PublishError::Domain(SandboxStoreError::NotFound))?;
    let current = read_sandbox(&row).map_err(PublishError::Domain)?;
    agents::validate_and_fence_scoped(store, &current, mutation, &mut scope).await?;
    // All captured evidence has been consumed. Remaining checkpoint, event and
    // Sandbox writes retain this transaction; no captured proof escapes release.
    publish_validated(store, mutation, current, scope.release()).await
}

async fn apply_publication(
    store: &MongoSandboxStore,
    mutation: &SandboxCheckpointMutation,
    session: &mut mongodb::ClientSession,
) -> Result<PublicationReceipt, PublishError> {
    crate::store::work_publication::fence_transaction(store, session)
        .await
        .map_err(|error| match error {
            crate::store::work_publication::FenceError::Domain(error) => {
                PublishError::Domain(error)
            }
            crate::store::work_publication::FenceError::Driver(error) => {
                PublishError::Driver(error)
            }
        })?;
    let current = store
        .sandboxes
        .find_one(doc! { "_id": mutation.guard.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(PublishError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(PublishError::Domain)?
        .ok_or(PublishError::Domain(SandboxStoreError::NotFound))?;
    agents::validate_and_fence(store, &current, mutation, session).await?;
    publish_validated(store, mutation, current, session).await
}

async fn publish_validated(
    store: &MongoSandboxStore,
    mutation: &SandboxCheckpointMutation,
    current: Sandbox,
    session: &mut mongodb::ClientSession,
) -> Result<PublicationReceipt, PublishError> {
    store
        .fence_checkpoint_owners(mutation, session)
        .await
        .map_err(|error| match error {
            crate::store::agent_ownership::FenceError::Domain(error) => PublishError::Domain(error),
            crate::store::agent_ownership::FenceError::Driver(error) => PublishError::Driver(error),
        })?;
    store
        .checkpoints
        .insert_one(
            checkpoint_document(
                &mutation.sandbox.id,
                mutation.sandbox.revision,
                &mutation.checkpoint,
            )
            .map_err(PublishError::Domain)?,
        )
        .session(&mut *session)
        .await
        .map_err(classify_write)?;
    prune_checkpoint_metadata(store, &mutation.sandbox, session).await?;
    let advanced = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(
                crate::store::work_publication::sandbox(
                    doc! {
                        "_id": current.id.as_str(),
                        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                        "channel_reuse_cleanup_execution": { "$exists": false },
                        "revision": signed(current.revision, "checkpoint sandbox revision")
                            .map_err(|_| PublishError::Domain(SandboxStoreError::Conflict))?,
                    },
                    current.id.as_str(),
                )
                .map_err(PublishError::Domain)?,
            ),
            crate::store::work_lease_update::replacement(
                sandbox_replacement_fields(&mutation.sandbox).map_err(PublishError::Domain)?,
            ),
        )
        .session(&mut *session)
        .await
        .map_err(classify_write)?;
    if advanced.matched_count != 1 {
        return Err(PublishError::Domain(SandboxStoreError::Conflict));
    }
    crate::store::sandbox_usage::save::record_saved(
        store,
        session,
        &mutation.sandbox,
        current.revision,
    )
    .await?;
    let event = append_in_transaction(
        store,
        session,
        &current.id,
        mutation.appended_at.clone(),
        SandboxEventKind::CheckpointSealed {
            object_reference: mutation.checkpoint.object_reference.clone(),
        },
    )
    .await?;
    // Archive reclamation shares this transaction so metadata and object
    // authority cannot disagree after a retry or ambiguous commit.
    for evicted in evicted_archives(
        &current.lifecycle.checkpoints,
        &mutation.sandbox.lifecycle.checkpoints,
    ) {
        prune_evicted(
            &store.checkpoint_objects,
            current.id.as_str(),
            evicted,
            session,
        )
        .await?;
    }
    Ok(PublicationReceipt {
        sandbox: mutation.sandbox.clone(),
        checkpoint: mutation.checkpoint.clone(),
        event,
    })
}
