use futures_util::TryStreamExt;
use layer2_core::StoreError;
use mongodb::bson::{doc, DateTime, Document};
use mongodb::options::ReadConcern;
use mongodb::{Collection, IndexModel};
use serde::{Deserialize, Serialize};

use super::model::ManifestDoc;
use super::{unavailable, AdapterResult, MongoCheckpointObjectStore};
use crate::store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome};
use crate::store::schema::DOCUMENT_SCHEMA_VERSION;

const PAGE_SIZE: i64 = 64;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Tombstone {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    object_key: String,
    next_attempt_at: DateTime,
    attempts: i64,
}

pub(super) async fn ensure_indexes(collection: &Collection<Document>) -> Result<(), StoreError> {
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! { "next_attempt_at": 1, "_id": 1 })
                .build(),
        )
        .await
        .map_err(|_| {
            StoreError::Unavailable("checkpoint cleanup tombstone indexes are not usable".into())
        })?;
    Ok(())
}

pub(super) async fn retain(
    store: &MongoCheckpointObjectStore,
    manifest: &ManifestDoc,
    now: DateTime,
) -> AdapterResult<bool> {
    let tombstone = Tombstone {
        id: manifest.id.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        object_key: manifest.object_key.clone(),
        next_attempt_at: next_attempt(now, 0),
        attempts: 0,
    };
    let mut session = store
        .client
        .start_session()
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone admission failed"))?;
    start_transaction(&mut session)
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone admission failed"))?;
    let deleted = store
        .manifests
        .delete_one(
            doc! { "_id": &manifest.id, "state": "deleting", "object_key": &manifest.object_key },
        )
        .session(&mut session)
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone admission failed"))?;
    if deleted.deleted_count != 1 {
        return Ok(false);
    }
    let document = mongodb::bson::serialize_to_document(&tombstone)
        .map_err(|_| unavailable("checkpoint cleanup tombstone encoding failed"))?;
    store
        .tombstones
        .insert_one(document)
        .session(&mut session)
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone admission failed"))?;
    match commit_transaction(&mut session).await {
        CommitOutcome::Committed => return Ok(true),
        CommitOutcome::Unknown(_) => (),
        _ => return Err(unavailable("checkpoint cleanup tombstone admission failed")),
    }
    let time = session.operation_time().ok_or_else(|| {
        unavailable("checkpoint cleanup tombstone commit has no reconciliation time")
    })?;
    let mut snapshot = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone reconciliation failed"))?;
    let retained = store
        .tombstones
        .find_one(doc! { "_id": &manifest.id })
        .read_concern(ReadConcern::snapshot())
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut snapshot)
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone reconciliation failed"))?
        .map(read)
        .transpose()?;
    let absent = store
        .manifests
        .find_one(doc! { "_id": &manifest.id })
        .read_concern(ReadConcern::snapshot())
        .max_time(std::time::Duration::from_secs(10))
        .session(&mut snapshot)
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone reconciliation failed"))?
        .is_none();
    if absent && retained.is_some_and(|value| value.object_key == manifest.object_key) {
        Ok(true)
    } else {
        Err(unavailable(
            "checkpoint cleanup tombstone admission is uncertain",
        ))
    }
}

pub(super) async fn cleanup_due(
    store: &MongoCheckpointObjectStore,
    now: DateTime,
) -> AdapterResult<usize> {
    let documents: Vec<Document> = store
        .tombstones
        .find(doc! { "next_attempt_at": { "$lte": now } })
        .sort(doc! { "next_attempt_at": 1, "_id": 1 })
        .limit(PAGE_SIZE)
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone scan failed"))?
        .try_collect()
        .await
        .map_err(|_| unavailable("checkpoint cleanup tombstone scan failed"))?;
    let mut completed = 0;
    let mut error = None;
    for document in documents {
        let tombstone = read(document)?;
        let result = store.objects.delete(&tombstone.object_key).await;
        let attempts = tombstone.attempts.saturating_add(1).min(20);
        // Back off both successful absence checks and failures. Ownership never expires.
        store.tombstones.update_one(doc! { "_id": &tombstone.id, "attempts": tombstone.attempts }, doc! {
            "$set": { "attempts": attempts, "next_attempt_at": next_attempt(now, attempts) },
        }).await.map_err(|_| unavailable("checkpoint cleanup tombstone retry scheduling failed"))?;
        match result {
            Ok(()) => completed += 1,
            Err(failure) => {
                error.get_or_insert(failure);
            }
        }
    }
    error.map_or(Ok(completed), Err)
}

fn next_attempt(now: DateTime, attempts: i64) -> DateTime {
    let seconds = (60_i64 * (1_i64 << attempts.clamp(0, 6))).min(3_600);
    DateTime::from_millis(now.timestamp_millis().saturating_add(seconds * 1_000))
}

fn read(document: Document) -> AdapterResult<Tombstone> {
    let value: Tombstone = mongodb::bson::deserialize_from_document(document)
        .map_err(|_| unavailable("checkpoint cleanup tombstone is corrupt"))?;
    let id = value.id.strip_prefix("cpa_");
    let key = value
        .object_key
        .strip_prefix("cma/checkpoints/")
        .and_then(|key| key.split_once('/'));
    let hex = |value: &str, length: usize| {
        value.len() == length
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    };
    if value.document_schema_version != DOCUMENT_SCHEMA_VERSION
        || !id.is_some_and(|id| hex(id, 32))
        || !key.is_some_and(|(owner, name)| hex(owner, 64) && name == format!("{}.cma", value.id))
        || !(0..=20).contains(&value.attempts)
        || value.next_attempt_at.timestamp_millis() <= 0
    {
        return Err(unavailable("checkpoint cleanup tombstone is corrupt"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_are_bounded_and_tombstones_reject_foreign_key_shapes() {
        let now = DateTime::from_millis(1_000);
        assert_eq!(next_attempt(now, 0).timestamp_millis(), 61_000);
        assert_eq!(next_attempt(now, 20).timestamp_millis(), 3_601_000);
        let id = format!("cpa_{}", "a".repeat(32));
        let valid = doc! { "_id": &id, "document_schema_version": DOCUMENT_SCHEMA_VERSION, "object_key": format!("cma/checkpoints/{}/{id}.cma", "b".repeat(64)), "next_attempt_at": now, "attempts": 0_i64 };
        assert!(read(valid.clone()).is_ok());
        for (field, value) in [
            ("object_key", "cma/checkpoints/other/object.cma"),
            ("_id", "foreign"),
        ] {
            let mut corrupt = valid.clone();
            corrupt.insert(field, value);
            assert!(read(corrupt).is_err());
        }
        let mut extra = valid;
        extra.insert("credential", "must-not-be-stored");
        assert!(read(extra).is_err());
    }
}
