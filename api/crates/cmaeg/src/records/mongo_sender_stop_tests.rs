use super::*;
use crate::records::mongo::{self, MongoRecords};
use mongodb::{
    options::{ClientOptions, Credential, IndexOptions},
    Client,
};

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo replica set"]
async fn sender_stop_reader_requires_versioned_catalog_and_exact_grants() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let db = admin.database(&format!("sender_stop_{}", uuid::Uuid::new_v4().simple()));
    let result = exercise(&uri, &db).await;
    db.drop().await.unwrap();
    result.unwrap();
}

async fn exercise(uri: &str, db: &Database) -> Result<(), Box<dyn std::error::Error>> {
    let records = RecordClient::new("https://records.invalid", "fixture")?;
    let name = db.name();
    crate::migrations::legacy(db, "--migrate").await?;
    db.run_command(mongo::role(name, false)).await?;
    db.run_command(role(name)).await?;
    let password = uuid::Uuid::new_v4().to_string();
    let connect = |user: &'static str, grants: Vec<&'static str>| {
        let password = password.clone();
        async move {
            db.run_command(doc! {"createUser":user,"pwd":&password,"roles":grants})
                .await?;
            let mut options = ClientOptions::parse(uri).await?;
            options.credential = Some(
                Credential::builder()
                    .username(user.to_owned())
                    .password(password)
                    .source(name.to_owned())
                    .build(),
            );
            MongoRecords::open(Client::with_options(options)?.database(name))
                .await
                .map_err(Box::<dyn std::error::Error>::from)
        }
    };
    let legacy = connect("legacy", vec!["cma_eg_runtime"]).await?;
    let grants = vec!["cma_eg_runtime", "cma_eg_sender_stop_runtime"];
    let first = records
        .clone()
        .with_database(connect("first", grants.clone()).await?);
    let second = records
        .clone()
        .with_database(connect("second", grants).await?);
    let old = records.with_database(legacy.clone());
    assert!(SenderStopRecords::open(&first).await.is_err());
    db.run_command(doc! {"collMod":MIGRATIONS,"validator":marker_validator()})
        .await?;
    db.create_collection(COLLECTION)
        .validator(validator())
        .await?;
    db.collection::<Document>(MIGRATIONS)
        .insert_one(doc! {"_id":MARKER,"version":VERSION})
        .await?;
    crate::migrations::legacy(db, "--migrate").await?;
    assert!(SenderStopRecords::open(&old).await.is_err());
    legacy.put("channel", "owner", b"channel", None).await?;
    assert_eq!(legacy.get("channel").await?.blob, b"channel");
    let a = SenderStopRecords::open(&first).await?;
    let b = SenderStopRecords::open(&second).await?;
    let id = "a".repeat(64);
    let raw = db.collection::<Document>(COLLECTION);
    let row = doc! {"_id":&id,"schema_version":VERSION,"revision":1_i64,
    "stage":"requested","sealed_evidence":Binary{ subtype:BinarySubtype::Generic, bytes:vec![1] }};
    raw.insert_one(row).await?;
    let (left, right) = tokio::try_join!(a.load(&id), b.load(&id))?;
    assert_eq!(left.unwrap().stage, SenderStopStage::Requested);
    assert_eq!(right.unwrap().stage, SenderStopStage::Requested);
    raw.create_index(
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
    assert!(SenderStopRecords::open(&first).await.is_err());
    Ok(())
}
