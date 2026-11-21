//! Runs inside the authenticated Mongo isolation contract with its runtime role.
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use cmaeg::{
    activity::ActivityStore,
    records::{
        mongo::{self, MongoRecords},
        RecordClient,
    },
};
use cmaeg_core::{
    ids::RecordId,
    seal::{seal_interaction, MasterKeyRing},
};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime, Document},
    Database,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub async fn verify(
    db: &Database,
    records: MongoRecords,
) -> Result<(), Box<dyn std::error::Error>> {
    let client =
        RecordClient::new("https://records.example", "unused-fixture")?.with_database(records);
    let keys = Arc::new(MasterKeyRing::parse(&format!(
        "1:{}",
        STANDARD.encode([7; 32])
    ))?);
    let store = ActivityStore::new(client.clone(), keys.clone());
    let owner = "activity-projection-owner";
    let record = uuid::Uuid::new_v4().to_string();
    let empty = store.latest(owner, &record).await?;
    assert!(store
        .latest_if_changed(owner, &record, Some(&empty.cursor))
        .await?
        .is_none());
    let mut cursor: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&empty.cursor)?)?;
    let day = cursor["day"].as_i64().unwrap();
    let kind = "channel_recent_activity_v1";
    let id = bucket_id(kind, owner, &record, day)?;
    let blob = seal_interaction(
        &keys,
        id.as_str(),
        &serde_json::to_vec(&json!({
            "kind":kind,"owner":owner,"record":record,"day":day,"has_more":false,
            "items":[{"event_id":"projected-event","source":{"type":"custom"},
                "actor":{"id":"producer","kind":"system"},
                "conversation":{"id":"job","kind":"custom"},
                "occurred_at":"2026-10-27T00:00:00Z","received_at":"2026-10-27T00:00:01Z",
                "consumers":[]}]
        }))?,
    )?;
    let expiry = DateTime::now().timestamp_millis() / 1000 + 3600;
    client
        .clone()
        .interactions()
        .put_expiring(&id, owner, &blob, None, Some(expiry))
        .await?;
    let page = store
        .latest_if_changed(owner, &record, Some(&empty.cursor))
        .await?
        .unwrap();
    assert_eq!(page.items[0].event_id, "projected-event");
    assert!(store
        .latest_if_changed(owner, &record, Some(&page.cursor))
        .await?
        .is_none());

    let collection = db.collection::<Document>(mongo::INTERACTIONS);
    // Corrupting only the ciphertext proves an unchanged watermark does not
    // fetch/decrypt buckets. A changed version must take the full validating read.
    collection
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$set":{"blob":Binary{
                subtype:BinarySubtype::Generic,bytes:b"invalid ciphertext".to_vec()
            }}},
        )
        .await?;
    assert!(store
        .latest_if_changed(owner, &record, Some(&page.cursor))
        .await?
        .is_none());
    assert!(store.latest(owner, &record).await.is_err());
    cursor = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&page.cursor)?)?;
    cursor["day"] = (day - 1).into();
    let previous_day = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor)?);
    assert!(store
        .latest_if_changed(owner, &record, Some(&previous_day))
        .await
        .is_err());
    collection
        .update_one(doc! {"_id":id.as_str()}, doc! {"$set":{"version":2_i64}})
        .await?;
    assert!(store
        .latest_if_changed(owner, &record, Some(&page.cursor))
        .await
        .is_err());
    collection
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$set":{"blob":Binary{
                subtype:BinarySubtype::Generic,bytes:blob
            }}},
        )
        .await?;
    let changed = store
        .latest_if_changed(owner, &record, Some(&page.cursor))
        .await?
        .unwrap();
    assert_ne!(changed.cursor, page.cursor);
    let rolled = store
        .latest_if_changed(owner, &record, Some(&previous_day))
        .await?
        .unwrap();
    assert_eq!(rolled.cursor, changed.cursor);
    // Forced reads remain available for lag/reset even without a changed cursor.
    assert!(store
        .latest_if_changed(owner, &record, None)
        .await?
        .is_some());

    for mutation in [
        doc! {"version":0_i64},
        doc! {"version":-1_i64},
        doc! {"version":"invalid"},
        doc! {"schema_version":2_i32},
        doc! {"expires_at":"invalid"},
        doc! {"owner":"forged"},
    ] {
        collection
            .update_one(doc! {"_id":id.as_str()}, doc! {"$set":mutation})
            .bypass_document_validation(true)
            .await?;
        assert!(store
            .latest_if_changed(owner, &record, Some(&changed.cursor))
            .await
            .is_err());
        collection
            .update_one(
                doc! {"_id":id.as_str()},
                doc! {"$set":{
                    "version":2_i64,"schema_version":1_i32,"owner":owner,
                    "expires_at":DateTime::from_millis(expiry * 1000)
                }},
            )
            .await?;
    }
    assert!(store
        .latest_if_changed(owner, &record, Some("malformed"))
        .await
        .is_err());
    cursor["day"] = day.into();
    cursor["versions"][0] = u64::MAX.into();
    let invalid_version = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor)?);
    assert!(store
        .latest_if_changed(owner, &record, Some(&invalid_version))
        .await
        .is_err());
    // Expiry invalidates the watermark even before Mongo's TTL monitor deletes it.
    collection
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$set":{
                "expires_at":DateTime::from_millis(1)
            }},
        )
        .await?;
    let expired = store
        .latest_if_changed(owner, &record, Some(&changed.cursor))
        .await?
        .unwrap();
    assert!(expired.items.is_empty());
    assert_eq!(expired.cursor, empty.cursor);

    // A write to the oldest retained bucket must also invalidate the watermark.
    let oldest_day = day - 6;
    let oldest = bucket_id(kind, owner, &record, oldest_day)?;
    let blob = seal_interaction(
        &keys,
        oldest.as_str(),
        &serde_json::to_vec(&json!({
            "kind":kind,"owner":owner,"record":record,"day":oldest_day,
            "has_more":false,"items":[]
        }))?,
    )?;
    client
        .interactions()
        .put_expiring(&oldest, owner, &blob, None, Some(expiry))
        .await?;
    let historical = store
        .latest_if_changed(owner, &record, Some(&empty.cursor))
        .await?
        .unwrap();
    let watermark: serde_json::Value =
        serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&historical.cursor)?)?;
    assert_eq!(watermark["versions"], json!([0, 0, 0, 0, 0, 0, 1]));
    assert!(store
        .latest_if_changed(owner, &record, Some(&historical.cursor))
        .await?
        .is_none());
    Ok(())
}

fn bucket_id(
    kind: &str,
    owner: &str,
    record: &str,
    day: i64,
) -> Result<RecordId, Box<dyn std::error::Error>> {
    let digest = Sha256::digest(serde_json::to_vec(&(kind, owner, record, day))?);
    let mut bytes: [u8; 16] = digest[..16].try_into()?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    Ok(RecordId::new(uuid::Uuid::from_bytes(bytes).to_string())?)
}
