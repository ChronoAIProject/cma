//! Run the real operator command with isolated and shared database credentials.
use base64::{engine::general_purpose::STANDARD, Engine};
use cma_service_auth::mongodb_authority::MongoAuthorityMode::SharedSchema;
use cmaeg::{
    activity::history::{schema, ActivityHistory, Config, CoverageGap, HistoryQuery, MongoHistory},
    records::{mongo, mongo::MongoRecords, RecordKind},
};
use cmaeg_core::{
    channel::ChannelDefinition,
    seal::{seal_blob, seal_interaction, MasterKeyRing},
};
use mongodb::{
    bson::{doc, Document},
    Client, Database,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{process::Output, sync::Arc};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

type Error = Box<dyn std::error::Error>;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn backfill_cli_honors_authority_and_preserves_legacy_and_foreign_state() {
    let uri = std::env::var("CMA_IM_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let db = admin.database(&format!("eg_backfill_{}", uuid::Uuid::new_v4().simple()));
    let result = exercise(&uri, &db).await;
    db.drop().await.unwrap();
    result.unwrap();
}

async fn user(uri: &str, db: &Database, name: &str, roles: Vec<Document>) -> Result<String, Error> {
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":name,"pwd":&password,"roles":roles})
        .await?;
    let mut url = url::Url::parse(uri)?;
    url.set_username(name).unwrap();
    url.set_password(Some(&password)).unwrap();
    let query: Vec<_> = url
        .query_pairs()
        .filter(|(key, _)| !key.eq_ignore_ascii_case("authSource"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    url.query_pairs_mut()
        .extend_pairs(query)
        .append_pair("authSource", db.name());
    Ok(url.into())
}

async fn command(uri: &str, name: &str, keys: &str, mode: Option<&str>) -> Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"));
    command
        .arg("--backfill-activity-history")
        .env_clear()
        .env("CMAEG_MONGODB_URI", uri)
        .env("CMAEG_MONGODB_DATABASE", name)
        .env("CMAEG_MASTER_KEYS", keys);
    if let Some(mode) = mode {
        command.env("CMAEG_MONGODB_AUTHORITY_MODE", mode);
    }
    tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .expect("bounded backfill command")
        .unwrap()
}

async fn exercise(uri: &str, db: &Database) -> Result<(), Error> {
    mongo::migrate(db).await?;
    schema::migrate(db).await?;
    let shared = user(
        uri,
        db,
        "shared",
        vec![
            doc! {"role":"readWrite","db":db.name()},
            doc! {"role":"dbAdmin","db":db.name()},
        ],
    )
    .await?;
    let runtime = Client::with_uri_str(&shared).await?.database(db.name());
    let records = MongoRecords::open_with_authority(runtime.clone(), SharedSchema).await?;
    let key_spec = format!("1:{}", STANDARD.encode([43; 32]));
    let keys = Arc::new(MasterKeyRing::parse(&key_spec)?);
    let (channel, received, expires_ms) = seed(&records, &keys).await?;
    for name in ["agents", "im_images"] {
        db.run_command(doc! {"create":name,"validator":{"owner":{"$type":"string"}},"validationLevel":"strict","validationAction":"error"}).await?;
        db.collection::<Document>(name)
            .insert_one(doc! {"_id":"foreign","owner":"another-service","payload":"preserve"})
            .await?;
        db.run_command(doc! {"createIndexes":name,"indexes":[{"key":{"owner":1},"name":"foreign_owner","unique":true}]}).await?;
    }
    let unchanged: Vec<_> = mongo::COLLECTIONS
        .iter()
        .copied()
        .chain(["agents", "im_images", schema::CONTROL])
        .collect();
    let before = super::shared_schema::snapshot(db, &unchanged).await?;
    let history_before = super::shared_schema::snapshot(db, schema::COLLECTIONS).await?;
    let collections_before = db.list_collection_names().await?;

    // The fixture has complete history/read schemas; refusals must be authority
    // failures, not missing schema or unusable sealed source data.
    db.run_command(mongo::role(db.name(), false)).await?;
    let history_reads: Vec<_> = schema::COLLECTIONS.iter().map(|name|
        doc! {"resource":{"db":db.name(),"collection":*name},"actions":["find","listIndexes"]}
    ).collect();
    db.run_command(doc! {"createRole":"history_reader","privileges":history_reads,"roles":[]})
        .await?;
    let readonly = user(
        uri,
        db,
        "no_history_writes",
        vec![
            doc! {"role":"cma_eg_runtime","db":db.name()},
            doc! {"role":"history_reader","db":db.name()},
        ],
    )
    .await?;
    let cross_database = user(
        uri,
        db,
        "cross_database",
        vec![
            doc! {"role":"readWrite","db":db.name()},
            doc! {"role":"dbAdmin","db":db.name()},
            doc! {"role":"read","db":format!("{}_other",db.name())},
        ],
    )
    .await?;
    let cluster = user(
        uri,
        db,
        "cluster_authority",
        vec![
            doc! {"role":"readWrite","db":db.name()},
            doc! {"role":"dbAdmin","db":db.name()},
            doc! {"role":"clusterMonitor","db":"admin"},
        ],
    )
    .await?;
    for (label, credential, mode) in [
        ("default isolated", &shared, None),
        ("explicit isolated", &shared, Some("isolated")),
        ("invalid mode", &shared, Some("shared")),
        ("missing history writes", &readonly, Some("shared-schema")),
        ("cross database", &cross_database, Some("shared-schema")),
        ("cluster authority", &cluster, Some("shared-schema")),
    ] {
        let output = command(credential, db.name(), &key_spec, mode).await;
        assert_eq!(output.status.code(), Some(1), "{label}");
        assert!(output.stdout.is_empty(), "{label}");
        assert_eq!(
            output.stderr, b"activity history administration failed; details withheld\n",
            "{label}"
        );
        assert_eq!(
            super::shared_schema::snapshot(db, schema::COLLECTIONS).await?,
            history_before,
            "{label}"
        );
        assert_eq!(
            super::shared_schema::snapshot(db, &unchanged).await?,
            before,
            "{label}"
        );
    }

    let mut imported = None;
    for _ in 0..2 {
        let output = command(&shared, db.name(), &key_spec, Some("shared-schema")).await;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            output.stdout,
            b"activity history backfill processed 1 surviving receipts; coverage remains partial\n"
        );
        let root = db
            .collection::<Document>(schema::EVENTS)
            .find_one(doc! {})
            .await?
            .unwrap();
        assert_eq!(
            root.get_datetime("expires_at")?.timestamp_millis(),
            expires_ms
        );
        assert_eq!(
            db.collection::<Document>(schema::EVENTS)
                .count_documents(doc! {})
                .await?,
            1
        );
        let after = super::shared_schema::snapshot(db, schema::COLLECTIONS).await?;
        if let Some(ref initial) = imported {
            assert_eq!(
                &after, initial,
                "repeat preserves history bytes, sequence and expiry"
            );
        }
        imported = Some(after);
        assert_eq!(
            super::shared_schema::snapshot(db, &unchanged).await?,
            before
        );
    }
    let mut collections_after = db.list_collection_names().await?;
    let mut collections_before = collections_before;
    collections_before.sort();
    collections_after.sort();
    assert_eq!(collections_after, collections_before);
    let history = ActivityHistory::new(
        Some(Arc::new(MongoHistory::open(runtime).await?)),
        keys,
        Config {
            enabled: true,
            ..Default::default()
        },
    );
    let page = history
        .page(
            &channel.owner.subject,
            channel.record_id.as_str(),
            HistoryQuery {
                from: Some((received - time::Duration::seconds(1)).format(&Rfc3339)?),
                until: Some((received + time::Duration::seconds(1)).format(&Rfc3339)?),
                ..Default::default()
            },
        )
        .await?;
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].activity.event_id, "fixture-event");
    assert!(page.items[0].legacy_partial);
    assert!(!page.coverage.complete_event_enumeration);
    assert!(page.coverage.gaps.contains(&CoverageGap::LegacyPartial));
    Ok(())
}

async fn seed(
    records: &MongoRecords,
    keys: &MasterKeyRing,
) -> Result<(ChannelDefinition, OffsetDateTime, i64), Error> {
    let mut channel: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../cmaeg-core/tests/fixtures/channel.json"
    ))?;
    channel.record_id = cmaeg_core::ids::RecordId::new(uuid::Uuid::new_v4().to_string())?;
    let owner = &channel.owner.subject;
    let record = channel.record_id.as_str();
    let blob = seal_blob(keys, channel.to_blob_json().to_string().as_bytes())?;
    records.put(record, owner, &blob, None).await?;
    // Yesterday's bucket expires before a new seven-day receipt would, so the
    // test detects backfill extending the retained source's lifetime.
    let received = (OffsetDateTime::now_utc() - time::Duration::days(1)).replace_nanosecond(0)?;
    let day = received.unix_timestamp().div_euclid(86400);
    let kind = "channel_recent_activity_v1";
    let digest = Sha256::digest(serde_json::to_vec(&(kind, owner, record, day))?);
    let mut bytes: [u8; 16] = digest[..16].try_into()?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let id = uuid::Uuid::from_bytes(bytes).to_string();
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/sdk-contract.json"))?;
    let mut activity = fixtures["ChannelActivity"][0].clone();
    activity["received_at"] = received.format(&Rfc3339)?.into();
    let bucket = json!({"kind":kind,"owner":owner,"record":record,"day":day,"has_more":false,"items":[activity]});
    let blob = seal_interaction(keys, &id, &serde_json::to_vec(&bucket)?)?;
    let expires_seconds = (day + 7) * 86400;
    records
        .for_kind(RecordKind::Interaction)
        .put_expiring(&id, owner, &blob, None, Some(expires_seconds))
        .await?;
    Ok((channel, received, expires_seconds * 1000))
}
