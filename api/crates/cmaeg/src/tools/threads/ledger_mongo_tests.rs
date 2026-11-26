//! Regression against actual Mongo validators, grants and the production thread ledger.
use super::*;
use crate::records::{
    mongo::{self, MongoRecords},
    RecordKind,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::{ClientOptions, Credential, IndexOptions},
    Client, Database, IndexModel,
};

#[path = "rename_mongo_tests.rs"]
mod rename;

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn persistent_thread_fences_survive_claim_expiry_and_restart_on_real_mongo() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!(
        "cma_thread_fence_contract_{}",
        uuid::Uuid::new_v4().simple()
    );
    let result = exercise(&uri, &admin.database(&name)).await;
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}

async fn exercise(uri: &str, db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let name = db.name();
    mongo::migrate(db).await?;
    for role in [
        mongo::role(name, false),
        mongo::thread_fences_role(name, false),
        mongo::thread_fences_role(name, true),
    ] {
        db.run_command(role).await?;
    }
    let password = uuid::Uuid::new_v4().to_string();
    for (username, role) in [
        ("runtime", "cma_eg_runtime"),
        ("fence-migrator", "cma_eg_thread_fences_migrator"),
    ] {
        db.run_command(
            doc! {"createUser":username,"pwd":&password,"roles":[{"role":role,"db":name}]},
        )
        .await?;
    }
    let connect = |username: &str| {
        let username = username.to_owned();
        let password = password.clone();
        async move {
            let mut options = ClientOptions::parse(uri).await?;
            options.credential = Some(
                Credential::builder()
                    .username(username)
                    .password(password)
                    .source(name.to_owned())
                    .build(),
            );
            Client::with_options(options)
        }
    };
    // An ordinary deployment still opens without optional collections or grants.
    let runtime = connect("runtime").await?;
    let base = MongoRecords::open(runtime.database(name)).await?;
    assert!(base.thread_fences_ready().await.is_err());
    db.run_command(doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_thread_fences_runtime","db":name}]}).await?;
    let runtime = connect("runtime").await?;
    let records = MongoRecords::open(runtime.database(name)).await?;
    assert!(records.thread_fences_ready().await.is_err());
    let migrator = connect("fence-migrator").await?;
    for _ in 0..2 {
        mongo::migrate_thread_fences(&migrator.database(name)).await?;
    }
    records.thread_fences_ready().await?;
    // The common validator preserves independent optional markers in either migration order.
    db.collection::<Document>(mongo::MIGRATIONS)
        .insert_one(doc! {"_id":"whatsapp_forms","version":1})
        .await?;
    mongo::migrate(db).await?;
    mongo::migrate_thread_fences(&migrator.database(name)).await?;
    records.thread_fences_ready().await?;
    for (marker, version) in [("schema", 2), ("thread_fences", 1), ("whatsapp_forms", 1)] {
        assert_eq!(
            db.collection::<Document>(mongo::MIGRATIONS)
                .find_one(doc! {"_id":marker})
                .await?
                .unwrap()
                .get_i32("version")?,
            version
        );
    }
    assert!(db
        .collection::<Document>(mongo::MIGRATIONS)
        .update_one(doc! {"_id":"thread_fences"}, doc! {"$set":{"version":2}})
        .await
        .is_err());
    let client =
        RecordClient::new("https://records.example", "fixture")?.with_database(records.clone());
    assert!(client.thread_fences_ready().await);
    exercise_ledger(db, &runtime.database(name), client.clone()).await?;
    let collection = db.collection::<Document>(mongo::THREAD_FENCES);
    // Missing markers/indexes or a TTL fault disable lifecycle without breaking base startup.
    db.collection::<Document>(mongo::MIGRATIONS)
        .delete_one(doc! {"_id":"thread_fences"})
        .await?;
    assert!(!client.thread_fences_ready().await);
    MongoRecords::open(runtime.database(name)).await?;
    mongo::migrate_thread_fences(&migrator.database(name)).await?;
    collection.drop_index("owner_1__id_1").await?;
    assert!(!client.thread_fences_ready().await);
    mongo::migrate_thread_fences(&migrator.database(name)).await?;
    collection
        .create_index(
            IndexModel::builder()
                .keys(doc! {"expires_at":1})
                .options(
                    IndexOptions::builder()
                        .expire_after(std::time::Duration::ZERO)
                        .build(),
                )
                .build(),
        )
        .await?;
    assert!(!client.thread_fences_ready().await);
    assert!(mongo::migrate_thread_fences(&migrator.database(name))
        .await
        .is_err());
    collection.drop_index("expires_at_1").await?;
    assert!(client.thread_fences_ready().await);
    let mut partial = mongo::thread_fences_role(name, false);
    partial.get_array_mut("privileges")?[0]
        .as_document_mut()
        .unwrap()
        .insert("actions", vec!["find", "insert", "listIndexes"]);
    db.run_command(doc! {"updateRole":"cma_eg_thread_fences_runtime",
    "privileges":partial.get_array("privileges")?.clone(),"roles":[]})
        .await?;
    let runtime = connect("runtime").await?;
    let records = MongoRecords::open(runtime.database(name)).await?;
    assert!(records.thread_fences_ready().await.is_err());
    Ok(())
}

async fn exercise_ledger(
    db: &Database,
    runtime: &Database,
    client: RecordClient,
) -> Result<(), Box<dyn std::error::Error>> {
    let keys = Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([3; 32]))).unwrap());
    let store = Store::new(client.clone(), keys.clone());
    let expiry = DateTime::now().timestamp_millis() / 1000 + 3600;
    let Admission::Claimed(claim) = store
        .claim(
            "event-one".into(),
            "key-one",
            "archive".into(),
            "owner",
            expiry,
        )
        .await
        .unwrap()
    else {
        panic!("fresh claim must succeed");
    };
    let claim_id = claim.id.clone();
    // This exact call fails against the old interactions-only ledger before any provider IO.
    let fence = store
        .acquire("77", "99", "owner")
        .await
        .unwrap()
        .expect("a permanent fence must be stored without Interaction expiry");
    let fence_id = fence.id.clone();
    let collection = db.collection::<Document>(mongo::THREAD_FENCES);
    let row = collection.find_one(doc! {"_id":&fence_id}).await?.unwrap();
    assert!(!row.contains_key("expires_at"));
    assert!(db
        .collection::<Document>(mongo::INTERACTIONS)
        .find_one(doc! {"_id":&fence_id})
        .await?
        .is_none());
    assert!(db
        .collection::<Document>(mongo::RECORDS)
        .find_one(doc! {"_id":&fence_id})
        .await?
        .is_none());
    let saved_claim = db
        .collection::<Document>(mongo::INTERACTIONS)
        .find_one(doc! {"_id":&claim_id})
        .await?
        .unwrap();
    assert_eq!(
        saved_claim.get_datetime("expires_at")?.timestamp_millis(),
        expiry * 1000
    );
    let id = RecordId::new(fence_id.clone())?;
    assert!(client
        .clone()
        .interactions()
        .put(&id, "owner", b"invalid", None)
        .await
        .is_err());
    let persistent = client.clone().thread_fences();
    assert!(persistent
        .put_expiring(&id, "owner", b"invalid", Some(1), Some(expiry))
        .await
        .is_err());
    assert!(persistent
        .put(&id, "other-owner", b"invalid", Some(1))
        .await
        .is_err());
    assert!(persistent.delete(&id, 1).await.is_err());
    let limited = runtime.collection::<Document>(mongo::THREAD_FENCES);
    assert!(limited.delete_one(doc! {"_id":&fence_id}).await.is_err());
    assert!(limited
        .update_one(
            doc! {"_id":&fence_id},
            doc! {"$set":{"expires_at":DateTime::now()}}
        )
        .await
        .is_err());
    assert!(runtime
        .run_command(doc! {"collMod":mongo::THREAD_FENCES,"validationLevel":"off"})
        .await
        .is_err());
    let unknown = ThreadReceipt {
        outcome: ThreadOutcome::Unknown {
            reason: "provider_write_uncertain".into(),
        },
        thread: None,
    };
    store.finish(claim, &unknown).await.unwrap();
    assert!(
        matches!(store.claim("event-one".into(),"key-one","archive".into(),"owner",expiry).await.unwrap(), Admission::Replay(receipt) if receipt == unknown)
    );
    // Simulate logical expiry and physical TTL cleanup without waiting an hour.
    db.collection::<Document>(mongo::INTERACTIONS)
        .update_one(
            doc! {"_id":&claim_id},
            doc! {"$set":{"expires_at":DateTime::from_millis(1)}},
        )
        .await?;
    assert!(matches!(
        client
            .clone()
            .interactions()
            .get(
                client
                    .clone()
                    .interactions()
                    .record_url(&claim_id)?
                    .as_str()
            )
            .await,
        Err(RecordError::NotFound)
    ));
    db.collection::<Document>(mongo::INTERACTIONS)
        .delete_one(doc! {"_id":&claim_id})
        .await?;
    let restarted_client = RecordClient::new("https://records.example", "fixture")?
        .with_database(MongoRecords::open(runtime.clone()).await?);
    let restarted = Store::new(restarted_client, keys);
    assert!(matches!(
        restarted
            .claim(
                "event-two".into(),
                "key-two",
                "reopen".into(),
                "owner",
                expiry
            )
            .await
            .unwrap(),
        Admission::Claimed(_)
    ));
    assert!(restarted
        .acquire("77", "99", "owner")
        .await
        .unwrap()
        .is_none());
    assert!(restarted.acquire("77", "99", "other-owner").await.is_err());
    assert!(restarted
        .acquire("78", "99", "owner")
        .await
        .unwrap()
        .is_some());
    // Confirmed operations release via CAS; racing new writes have one winner.
    let completed = restarted
        .acquire("77", "100", "owner")
        .await
        .unwrap()
        .unwrap();
    let stale: Fence = serde_json::from_value(serde_json::to_value(&completed).unwrap()).unwrap();
    let stale = Fence {
        version: completed.version,
        ..stale
    };
    restarted.release(completed).await.unwrap();
    let (left, right) = tokio::join!(
        store.acquire("77", "100", "owner"),
        restarted.acquire("77", "100", "owner")
    );
    assert_eq!(
        usize::from(left.unwrap().is_some()) + usize::from(right.unwrap().is_some()),
        1
    );
    assert!(restarted.release(stale).await.is_err());
    assert!(store.acquire("77", "100", "owner").await.unwrap().is_none());
    // No successful release or a different event can clear the original ambiguous fence.
    assert!(store.acquire("77", "99", "owner").await.unwrap().is_none());
    assert_eq!(
        collection
            .find_one(doc! {"_id":&fence_id})
            .await?
            .unwrap()
            .get_i64("version")?,
        1
    );
    assert!(
        client
            .clone()
            .with_database(MongoRecords::open(runtime.clone()).await?)
            .thread_fences_ready()
            .await
    );
    assert_eq!(RecordKind::ThreadFence.collection(), mongo::THREAD_FENCES);
    Ok(())
}
