//! Real grants, TTL visibility and restart behavior for optional scheduling hints.
use super::*;
use crate::records::{RecordClient, RecordKind};
use mongodb::{
    bson::DateTime,
    options::{ClientOptions, Credential},
    Client,
};

#[test]
fn media_continuation_indexes_refuse_partial_sparse_hidden_and_non_expiring_heads() {
    let valid = vec![
        IndexModel::builder()
            .keys(doc! {"_id":1})
            .options(IndexOptions::builder().name("_id_".to_owned()).build())
            .build(),
        IndexModel::builder()
            .keys(doc! {"owner":1,"_id":1})
            .options(
                IndexOptions::builder()
                    .name("owner_1__id_1".to_owned())
                    .build(),
            )
            .build(),
        IndexModel::builder()
            .keys(doc! {"expires_at":1})
            .options(
                IndexOptions::builder()
                    .name("expires_at_1".to_owned())
                    .expire_after(Duration::ZERO)
                    .build(),
            )
            .build(),
    ];
    assert!(indexes_valid(&valid));
    assert!(!indexes_valid(&valid[..2]));
    for options in [
        IndexOptions::builder()
            .name("unexpected_name".to_owned())
            .build(),
        IndexOptions::builder()
            .name("owner_1__id_1".to_owned())
            .partial_filter_expression(doc! {"owner":"x"})
            .build(),
        IndexOptions::builder()
            .name("owner_1__id_1".to_owned())
            .sparse(true)
            .build(),
        IndexOptions::builder()
            .name("owner_1__id_1".to_owned())
            .hidden(true)
            .build(),
        IndexOptions::builder()
            .name("owner_1__id_1".to_owned())
            .unique(true)
            .build(),
    ] {
        let mut bad = valid.clone();
        bad[1].options = Some(options);
        assert!(!indexes_valid(&bad));
    }
    let mut bad = valid.clone();
    bad[2].options.as_mut().unwrap().expire_after = Some(Duration::from_secs(1));
    assert!(!indexes_valid(&bad));
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn optional_media_continuation_storage_has_bounded_scans_original_expiry_and_exact_grants() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_media_heads_{}", uuid::Uuid::new_v4().simple());
    let result = exercise(&uri, &admin.database(&name)).await;
    admin.database(&name).drop().await.unwrap();
    result.unwrap();
}
async fn exercise(uri: &str, db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let name = db.name();
    super::super::migrate(db).await?;
    db.run_command(super::super::role(name, false)).await?;
    db.run_command(role(name, false)).await?;
    db.run_command(role(name, true)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(
        doc! {"createUser":"runtime","pwd":&password,"roles":[{"role":"cma_eg_runtime","db":name}]},
    )
    .await?;
    db.run_command(doc! {"createUser":"migrator","pwd":&password,"roles":[{"role":"cma_eg_media_continuations_migrator","db":name}]}).await?;
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
    let runtime = connect("runtime").await?;
    let base = MongoRecords::open(runtime.database(name)).await?;
    assert!(base.media_continuations_ready().await.is_err());
    let migrator = connect("migrator").await?;
    migrate(&migrator.database(name)).await?;
    migrate(&migrator.database(name)).await?;
    MongoRecords::open(runtime.database(name)).await?;
    assert!(base.media_continuations_ready().await.is_err());
    assert!(base
        .for_kind(RecordKind::MediaContinuation)
        .list(None, None, 32)
        .await
        .is_err());
    db.run_command(doc! {"grantRolesToUser":"runtime","roles":[{"role":"cma_eg_media_continuations_runtime","db":name}]}).await?;
    let runtime = connect("runtime").await?;
    let records = MongoRecords::open(runtime.database(name)).await?;
    records.media_continuations_ready().await?;
    let client = RecordClient::new("https://manager.example", "fixture")?
        .with_database(records.clone())
        .media_continuations();
    assert!(client.media_continuations_ready().await);
    let heads = records.for_kind(RecordKind::MediaContinuation);
    let retained_until = DateTime::now().timestamp_millis() / 1000 + 3600;
    let mut ids: Vec<String> = (0..35).map(|_| uuid::Uuid::new_v4().to_string()).collect();
    ids.sort();
    for (i, id) in ids.iter().enumerate() {
        let owner = if i == 34 { "other" } else { "owner" };
        assert_eq!(
            heads
                .put_expiring(id, owner, b"sealed-plan", None, Some(retained_until))
                .await?,
            1
        );
    }
    let first = heads.list(None, None, 32).await?;
    assert_eq!(
        first
            .iter()
            .map(|head| head.record_id.clone())
            .collect::<Vec<_>>(),
        ids[..32]
    );
    let restarted = MongoRecords::open(runtime.database(name))
        .await?
        .for_kind(RecordKind::MediaContinuation);
    let remaining = restarted.list(None, Some(&first[31].record_id), 32).await?;
    assert_eq!(
        remaining
            .iter()
            .map(|head| head.record_id.clone())
            .collect::<Vec<_>>(),
        ids[32..]
    );
    assert_eq!(
        heads.list(Some("other"), None, 32).await?[0].record_id,
        ids[34]
    );
    assert!(heads.list(None, None, 33).await.is_err());
    assert!(heads.list(None, None, 0).await.is_err());
    assert!(heads
        .put(
            &uuid::Uuid::new_v4().to_string(),
            "owner",
            b"missing-expiry",
            None
        )
        .await
        .is_err());
    assert!(heads
        .put_expiring(&ids[0], "other", b"stolen", Some(1), Some(retained_until))
        .await
        .is_err());
    assert!(heads
        .put_expiring(
            &ids[0],
            "owner",
            b"extended",
            Some(1),
            Some(retained_until + 1)
        )
        .await
        .is_err());
    assert_eq!(
        heads
            .put_expiring(&ids[0], "owner", b"progress", Some(1), Some(retained_until))
            .await?,
        2
    );
    assert_eq!(heads.get(&ids[0]).await?.blob, b"progress");
    let raw = db.collection::<Document>(MEDIA_CONTINUATIONS);
    assert_eq!(
        raw.find_one(doc! {"_id":&ids[0]})
            .await?
            .unwrap()
            .get_datetime("expires_at")?
            .timestamp_millis(),
        retained_until * 1000
    );
    // Expiry takes effect without waiting for Mongo's TTL deletion cycle.
    raw.update_one(
        doc! {"_id":&ids[1]},
        doc! {"$set":{"expires_at":DateTime::from_millis(DateTime::now().timestamp_millis()-1)}},
    )
    .await?;
    assert!(matches!(
        heads.get(&ids[1]).await,
        Err(RecordError::NotFound)
    ));
    assert!(!heads
        .list(None, None, 32)
        .await?
        .iter()
        .any(|head| head.record_id == ids[1]));
    assert!(heads.delete(&ids[0], 1).await.is_err());
    assert!(heads.delete(&ids[0], 2).await?);
    assert!(matches!(
        heads.get(&ids[0]).await,
        Err(RecordError::NotFound)
    ));
    assert!(runtime
        .database(name)
        .run_command(doc! {"collMod":MEDIA_CONTINUATIONS,"validationLevel":"off"})
        .await
        .is_err());
    assert!(runtime
        .database(name)
        .collection::<Document>("agents")
        .find_one(doc! {})
        .await
        .is_err());
    super::super::migrate_thread_fences(db).await?;
    super::super::migrate_whatsapp_flows(db).await?;
    super::super::migrate(db).await?;
    migrate(&migrator.database(name)).await?;
    for (marker, version) in [
        ("schema", 2),
        ("thread_fences", 1),
        ("whatsapp_forms", 1),
        ("media_continuations", 1),
    ] {
        assert_eq!(
            db.collection::<Document>(MIGRATIONS)
                .find_one(doc! {"_id":marker})
                .await?
                .unwrap()
                .get_i32("version")?,
            version
        );
    }
    records.media_continuations_ready().await?;
    raw.drop_index("expires_at_1").await?;
    assert!(!client.media_continuations_ready().await);
    migrate(&migrator.database(name)).await?;
    assert!(client.media_continuations_ready().await);
    raw.drop_index("expires_at_1").await?;
    raw.create_index(
        IndexModel::builder()
            .keys(doc! {"expires_at":1})
            .options(
                IndexOptions::builder()
                    .expire_after(Duration::ZERO)
                    .partial_filter_expression(doc! {"owner":"owner"})
                    .build(),
            )
            .build(),
    )
    .await?;
    assert!(!client.media_continuations_ready().await);
    assert!(migrate(&migrator.database(name)).await.is_err());
    Ok(())
}
