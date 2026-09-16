use super::*;
use layer2_core::bridge::outbox::OutboxError;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn channel_control_reference_unknown_commit_recovers_original_random_value_and_consent() {
    let f = ReplyFixture::new().await;
    consent(&f, 0, SourceControlAction::AllowSenderCancelAndSteer).await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app_name = format!("cma-reference-{}", uuid::Uuid::new_v4().simple());
    let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = mongodb::Client::with_options(options).unwrap();
    let resources = Arc::new(
        crate::store::MongoSandboxStore::open(&client, &f.database)
            .await
            .unwrap(),
    );
    let bridges = Arc::new(
        crate::store::MongoBridgeStore::open(
            &client,
            &f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap(),
    );
    let invokes = Arc::new(
        crate::store::MongoBridgeInvokeStore::open(bridges, resources, &f.database)
            .await
            .unwrap(),
    );
    let references = crate::store::MongoBridgeReplyToolStore::new(invokes, Arc::new(Authority))
        .await
        .unwrap();
    let session = f.tools.authenticate(&f.credential).await.unwrap();
    let admin = mongodb::Client::with_uri_str(&uri).await.unwrap();
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":app_name,
            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected ambiguous reference commit"}}
    }).await.unwrap();
    let result = references.control_context(&session).await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert_eq!(result, Err(OutboxError::Unavailable));
    let original = rows(&f).await;
    assert_eq!(
        original.len(),
        1,
        "the first commit actually persisted its random mapping"
    );
    let retained = record(&original[0]);
    let recovered = read(&f).await;
    assert_eq!(recovered["source_control_reference"], retained.reference);
    assert_eq!(rows(&f).await, original);
    assert!(f.items().await.is_empty());
    f.close().await;
}
