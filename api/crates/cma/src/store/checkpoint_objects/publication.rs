use mongodb::bson::{doc, DateTime, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::model::{find_manifest, manifest_document, read_manifest, ArchiveIdentity, ManifestDoc};
use super::{rejected, unavailable, AdapterResult, MongoCheckpointObjectStore};
use crate::store::sandbox_mongo::{
    commit_transaction, start_transaction, CommitOutcome, DOCUMENT_SCHEMA_VERSION,
};

async fn touch_gate(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
    session: &mut ClientSession,
) -> AdapterResult<()> {
    let touched = store.gates.update_one(doc! { "_id": sandbox_id, "document_schema_version": DOCUMENT_SCHEMA_VERSION, "retired": false }, doc! { "$inc": { "serial": 1_i64 } })
        .session(session).await.map_err(|_| unavailable("checkpoint archive writer gate is unavailable"))?;
    if touched.matched_count == 1 {
        Ok(())
    } else {
        Err(rejected("checkpoint archive sandbox has been retired"))
    }
}

pub(super) async fn admit(
    store: &MongoCheckpointObjectStore,
    staged: &ManifestDoc,
) -> AdapterResult<()> {
    store.gates.update_one(doc! { "_id": &staged.sandbox_id }, doc! { "$setOnInsert": { "document_schema_version": DOCUMENT_SCHEMA_VERSION, "retired": false, "serial": 0_i64 } })
        .upsert(true).await.map_err(|_| unavailable("checkpoint archive gate initialization failed"))?;
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| unavailable("checkpoint archive admission is unavailable"))?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable("checkpoint archive admission is unavailable"))?;
    touch_gate(store, &staged.sandbox_id, &mut session).await?;
    store
        .manifests
        .insert_one(manifest_document(staged)?)
        .session(&mut session)
        .await
        .map_err(|_| unavailable("checkpoint archive admission failed"))?;
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => return Ok(()),
        CommitOutcome::Unknown(_) => (),
        _ => return Err(unavailable("checkpoint archive admission failed")),
    }
    let recorded = read_committed_stage(store, &staged.id, session.operation_time()).await?;
    if recorded.as_ref() == Some(staged) {
        Ok(())
    } else {
        Err(unavailable("checkpoint archive admission is uncertain"))
    }
}

pub(super) async fn publish(
    store: &MongoCheckpointObjectStore,
    staged: &ManifestDoc,
    identity: &ArchiveIdentity,
) -> AdapterResult<()> {
    let result = publish_transaction(store, staged, identity).await;
    if matches!(result, Err(layer3_adapter::AdapterError::Rejected(_))) {
        return result;
    }
    verify_active(store, &staged.sandbox_id).await?;
    let winner = find_manifest(store, &staged.sandbox_id, &staged.object_reference).await?;
    match winner {
        Some(winner) if winner.matches(&staged.sandbox_id, identity) => {
            if winner.id != staged.id {
                retire_unused(store, staged).await?;
            }
            Ok(())
        }
        Some(_) => Err(rejected(
            "checkpoint archive reference has conflicting metadata",
        )),
        None => result.and(Err(unavailable(
            "checkpoint archive publication is uncertain",
        ))),
    }
}

pub(super) async fn verify_active(
    store: &MongoCheckpointObjectStore,
    sandbox_id: &str,
) -> AdapterResult<()> {
    let active = store.gates.find_one(doc! { "_id": sandbox_id, "document_schema_version": DOCUMENT_SCHEMA_VERSION, "retired": false }).await
        .map_err(|_| unavailable("checkpoint archive writer gate is unavailable"))?;
    if active.is_some() {
        Ok(())
    } else {
        Err(rejected("checkpoint archive sandbox has been retired"))
    }
}

async fn publish_transaction(
    store: &MongoCheckpointObjectStore,
    staged: &ManifestDoc,
    identity: &ArchiveIdentity,
) -> AdapterResult<()> {
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| unavailable("checkpoint archive publication is unavailable"))?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable("checkpoint archive publication is unavailable"))?;
    touch_gate(store, &staged.sandbox_id, &mut session).await?;
    let existing = store.manifests.find_one(doc! { "sandbox_id": &staged.sandbox_id, "object_reference": &staged.object_reference, "state": "published" })
        .session(&mut session).await.map_err(|_| unavailable("checkpoint archive publication lookup failed"))?.map(read_manifest).transpose()?;
    let state = if let Some(existing) = existing {
        if !existing.matches(&staged.sandbox_id, identity) {
            return Err(rejected(
                "checkpoint archive reference has conflicting metadata",
            ));
        }
        "deleting"
    } else {
        "published"
    };
    let changed = store
        .manifests
        .update_one(
            doc! { "_id": &staged.id, "state": "uploading" },
            doc! { "$set": { "state": state, "upload_confirmed": true, "cleanup_after": DateTime::now() } },
        )
        .session(&mut session)
        .await
        .map_err(|_| unavailable("checkpoint archive publication failed"))?;
    if changed.matched_count != 1 {
        return Err(rejected(
            "checkpoint archive upload is no longer publishable",
        ));
    }
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => Ok(()),
        CommitOutcome::Unknown(_) => {
            let recorded =
                read_committed_stage(store, &staged.id, session.operation_time()).await?;
            if recorded.is_some_and(|recorded| {
                recorded.matches(&staged.sandbox_id, identity)
                    && recorded.upload_confirmed
                    && ((state == "published"
                        && recorded.state == super::model::ArchiveState::Published)
                        || (state == "deleting"
                            && recorded.state == super::model::ArchiveState::Deleting))
            }) {
                Ok(())
            } else {
                Err(unavailable("checkpoint archive publication is uncertain"))
            }
        }
        _ => Err(unavailable("checkpoint archive publication is uncertain")),
    }
}

async fn read_committed_stage(
    store: &MongoCheckpointObjectStore,
    id: &str,
    operation_time: Option<Timestamp>,
) -> AdapterResult<Option<ManifestDoc>> {
    let time = operation_time
        .ok_or_else(|| unavailable("checkpoint archive commit has no reconciliation time"))?;
    let mut snapshot = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(|_| unavailable("checkpoint archive commit reconciliation failed"))?;
    store
        .manifests
        .find_one(doc! { "_id": id })
        .read_concern(ReadConcern::snapshot())
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut snapshot)
        .await
        .map_err(|_| unavailable("checkpoint archive commit reconciliation failed"))?
        .map(read_manifest)
        .transpose()
}

async fn retire_unused(
    store: &MongoCheckpointObjectStore,
    staged: &ManifestDoc,
) -> AdapterResult<()> {
    store
        .manifests
        .update_one(
            doc! { "_id": &staged.id, "state": "uploading" },
            doc! { "$set": { "state": "deleting", "upload_confirmed": true, "cleanup_after": DateTime::now() } },
        )
        .await
        .map_err(|_| unavailable("checkpoint archive unused upload retirement failed"))?;
    Ok(())
}
