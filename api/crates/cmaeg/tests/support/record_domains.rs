//! Database transport must preserve the same domains and expiry as HTTP storage.
use cmaeg::records::{
    mongo::{self, MongoRecords},
    RecordClient, RecordError, RecordKind,
};
use cmaeg_core::ids::RecordId;
use mongodb::{
    bson::{doc, DateTime, Document},
    Database,
};

#[path = "activity_projection.rs"]
mod activity_projection;

pub async fn verify(
    db: &Database,
    records: MongoRecords,
) -> Result<(), Box<dyn std::error::Error>> {
    let channels = RecordClient::new("https://records.example", "unused-fixture")?
        .with_database(records.clone());
    let connections = channels.clone().connections();
    // Selection must also work when with_database comes after the domain.
    let interactions = RecordClient::new("https://records.example", "unused-fixture")?
        .interactions()
        .with_database(records.clone());
    let id = RecordId::new(uuid::Uuid::new_v4().to_string())?;
    let expiry = DateTime::now().timestamp_millis() / 1000 + 3600;
    assert_eq!(
        channels.put(&id, "channel-owner", b"channel", None).await?,
        1
    );
    assert_eq!(
        connections
            .put(&id, "connection-owner", b"connection", None)
            .await?,
        1
    );
    assert_eq!(
        interactions
            .put_expiring(&id, "interaction-owner", b"interaction", None, Some(expiry))
            .await?,
        1
    );
    for (client, owner, blob) in [
        (&channels, "channel-owner", b"channel".as_slice()),
        (&connections, "connection-owner", b"connection".as_slice()),
        (
            &interactions,
            "interaction-owner",
            b"interaction".as_slice(),
        ),
    ] {
        assert_eq!(
            client
                .get(client.record_url(id.as_str())?.as_str())
                .await?
                .blob,
            blob
        );
        let list = client.list(Some(owner), None, 100).await?;
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].record_id, id.as_str());
    }
    assert!(channels
        .list(Some("connection-owner"), None, 100)
        .await?
        .is_empty());
    assert!(channels
        .get(connections.record_url(id.as_str())?.as_str())
        .await
        .is_err());
    assert!(interactions
        .put(&id, "interaction-owner", b"missing-expiry", Some(1))
        .await
        .is_err());
    for invalid in [0, i64::MAX, expiry + 8 * 86400] {
        assert!(interactions
            .put_expiring(&id, "interaction-owner", b"bad", Some(1), Some(invalid))
            .await
            .is_err());
    }
    assert!(channels
        .put_expiring(&id, "channel-owner", b"bad", Some(1), Some(expiry))
        .await
        .is_err());
    assert!(interactions
        .put_expiring(
            &id,
            "interaction-owner",
            b"extend",
            Some(1),
            Some(expiry + 1)
        )
        .await
        .is_err());
    assert!(interactions
        .put_expiring(&id, "forged-owner", b"forged", Some(1), Some(expiry))
        .await
        .is_err());
    assert_eq!(
        interactions
            .put_expiring(&id, "interaction-owner", b"updated", Some(1), Some(expiry))
            .await?,
        2
    );
    assert!(connections.delete(&id, 1).await?);
    assert!(matches!(
        connections
            .get(connections.record_url(id.as_str())?.as_str())
            .await,
        Err(RecordError::NotFound)
    ));
    assert_eq!(
        channels
            .get(channels.record_url(id.as_str())?.as_str())
            .await?
            .blob,
        b"channel"
    );
    let interaction = db
        .collection::<Document>(mongo::INTERACTIONS)
        .find_one(doc! {"_id":id.as_str()})
        .await?
        .unwrap();
    assert_eq!(
        interaction.get_datetime("expires_at")?.timestamp_millis(),
        expiry * 1000
    );
    db.collection::<Document>(mongo::INTERACTIONS)
        .update_one(
            doc! {"_id":id.as_str()},
            doc! {"$set":{"expires_at":DateTime::from_millis(1)}},
        )
        .await?;
    assert!(matches!(
        interactions
            .get(interactions.record_url(id.as_str())?.as_str())
            .await,
        Err(RecordError::NotFound)
    ));
    assert!(interactions
        .list(Some("interaction-owner"), None, 100)
        .await?
        .is_empty());
    assert!(!interactions.delete(&id, 2).await?);
    assert!(interactions
        .put_expiring(&id, "interaction-owner", b"revive", Some(2), Some(expiry))
        .await
        .is_err());
    assert!(channels.delete(&id, 1).await?);
    verify_import(db, &records, expiry).await?;
    activity_projection::verify(db, records).await?;
    Ok(())
}

async fn verify_import(
    db: &Database,
    records: &MongoRecords,
    expiry: i64,
) -> Result<(), Box<dyn std::error::Error>> {
    let id = uuid::Uuid::new_v4().to_string();
    let expired = uuid::Uuid::new_v4().to_string();
    let path = std::env::temp_dir().join(format!("cma-eg-import-{}", uuid::Uuid::new_v4()));
    let rows = [
        serde_json::json!({"record_id":id,"owner":"legacy-channel","version":"7","blob":"Y2hhbm5lbA=="}),
        serde_json::json!({"kind":"connection","record_id":id,"owner":"legacy-connection","version":"8","blob":"Y29ubmVjdGlvbg=="}),
        serde_json::json!({"kind":"interaction","record_id":id,"owner":"legacy-interaction","version":"9","blob":"aW50ZXJhY3Rpb24=","expires_at":expiry}),
        serde_json::json!({"kind":"interaction","record_id":expired,"owner":"legacy-expired","version":"3","blob":"ZXhwaXJlZA==","expires_at":1}),
    ];
    std::fs::write(&path, rows.map(|row| row.to_string()).join("\n"))?;
    // Opaque record mechanics only; the GitHub write gate has its own contract.
    let gate = cmaeg::records::ImportGate::new(true, None);
    for _ in 0..2 {
        assert_eq!(
            cmaeg::records::import::import_with(db, &path, &gate).await?,
            4
        );
    }
    for (kind, version, blob) in [
        (RecordKind::Channel, 7, b"channel".as_slice()),
        (RecordKind::Connection, 8, b"connection".as_slice()),
        (RecordKind::Interaction, 9, b"interaction".as_slice()),
    ] {
        let observed = records.for_kind(kind).get(&id).await?;
        assert_eq!(observed.version, version);
        assert_eq!(observed.blob, blob);
    }
    assert!(db
        .collection::<Document>(mongo::INTERACTIONS)
        .find_one(doc! {"_id":&expired})
        .await?
        .is_none());
    assert_eq!(
        db.collection::<Document>(mongo::INTERACTIONS)
            .find_one(doc! {"_id":&id})
            .await?
            .unwrap()
            .get_datetime("expires_at")?
            .timestamp_millis(),
        expiry * 1000
    );
    records
        .for_kind(RecordKind::Connection)
        .put(&id, "legacy-connection", b"changed", Some(8))
        .await?;
    assert!(cmaeg::records::import::import_with(db, &path, &gate)
        .await
        .is_err());
    assert_eq!(
        records
            .for_kind(RecordKind::Connection)
            .get(&id)
            .await?
            .blob,
        b"changed"
    );
    for row in [
        serde_json::json!({"kind":"interaction","record_id":id,"owner":"legacy","version":"1","blob":"eA=="}),
        serde_json::json!({"kind":"channel","record_id":id,"owner":"legacy","version":"1","blob":"eA==","expires_at":expiry}),
        serde_json::json!({"kind":"unknown","record_id":id,"owner":"legacy","version":"1","blob":"eA=="}),
    ] {
        std::fs::write(&path, row.to_string())?;
        assert!(cmaeg::records::import::import_with(db, &path, &gate)
            .await
            .is_err());
    }
    std::fs::remove_file(path)?;
    Ok(())
}
