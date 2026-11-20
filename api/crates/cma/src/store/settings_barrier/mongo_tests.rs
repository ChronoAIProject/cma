use super::*;
use crate::{
    auth::registration::RegistrationCapacity,
    store::sandbox_mongo::{commit_transaction, start_transaction, CommitOutcome},
};
use layer2_core::{PlatformSettingsStore, SubjectId};
use std::sync::Arc;
use test_support::{platform, Pause, Point};

async fn fixture() -> (mongodb::Client, String, Collection<Document>) {
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = format!("cma_settings_barrier_{}", uuid::Uuid::new_v4().simple());
    crate::store::MongoRegistrationCapacityStore::open(&client, &name, false)
        .await
        .unwrap();
    let collection = client.database(&name).collection("platform_settings");
    (client, name, collection)
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn missing_settings_bootstrap_preserves_defaults_revision_and_rollback() {
    for enabled in [false, true] {
        let (client, name, settings) = fixture().await;
        let initial = InitialSettings::new(enabled);
        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let protected = acquire(&settings, initial, &mut session).await.unwrap();
        assert_eq!(protected.settings(), &initial.platform());
        session.abort_transaction().await.unwrap();
        assert_eq!(settings.count_documents(doc! {}).await.unwrap(), 0);
        let registrations = crate::store::MongoRegistrationCapacityStore::open_with_defaults(
            &client, &name, false, initial,
        )
        .await
        .unwrap();
        assert!(registrations
            .admit(&SubjectId::new("before-activation"))
            .await
            .unwrap());
        let stored = platform(&client, &name).await;
        assert_eq!(stored.get_i64("revision").unwrap(), 0);
        assert_eq!(stored.get_i64("admission_policy_fence").unwrap(), 1);
        assert_eq!(
            crate::store::settings::read_settings(stored.clone())
                .unwrap()
                .settings,
            initial.platform()
        );
        assert_eq!(
            settings.count_documents(doc! {}).await.unwrap(),
            1,
            "no invented settings history"
        );
        assert_eq!(
            client
                .database(&name)
                .collection::<Document>("activity")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            client
                .database(&name)
                .collection::<Document>("sandbox_admissions")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        // An opposite startup default cannot modify an existing document.
        start_transaction(&mut session).await.unwrap();
        let protected = acquire(&settings, InitialSettings::new(!enabled), &mut session)
            .await
            .unwrap();
        assert_eq!(protected.settings(), &initial.platform());
        assert!(matches!(
            commit_transaction(&mut session).await,
            CommitOutcome::Committed
        ));
        let after = platform(&client, &name).await;
        assert_eq!(after.get_i64("revision").unwrap(), 0);
        assert_eq!(after.get_i64("admission_policy_fence").unwrap(), 2);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires a disposable Mongo replica set"]
async fn invalid_envelopes_and_fence_values_are_refused_without_repair() {
    use mongodb::bson::Bson;
    let (client, name, settings) = fixture().await;
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    acquire(&settings, InitialSettings::default(), &mut session)
        .await
        .unwrap();
    assert!(matches!(
        commit_transaction(&mut session).await,
        CommitOutcome::Committed
    ));
    let valid = platform(&client, &name).await;
    for (field, value) in [
        ("admission_policy_fence", Bson::Int64(-1)),
        ("admission_policy_fence", Bson::Int64(i64::MAX)),
        ("admission_policy_fence", Bson::Double(1.0)),
        ("admission_policy_fence", Bson::Null),
        ("admission_policy_fence", Bson::String("broken".into())),
        ("document_schema_version", Bson::Int64(99)),
        ("revision", Bson::Int64(-1)),
        ("settings", Bson::Boolean(false)),
    ] {
        let mut invalid = valid.clone();
        invalid.insert(field, value);
        settings
            .replace_one(doc! {"_id": "platform"}, &invalid)
            .await
            .unwrap();
        start_transaction(&mut session).await.unwrap();
        assert!(matches!(
            acquire(&settings, InitialSettings::default(), &mut session).await,
            Err(Error::Unreadable)
        ));
        session.abort_transaction().await.unwrap();
        assert_eq!(platform(&client, &name).await, invalid);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires a disposable Mongo replica set"]
async fn operator_cannot_acknowledge_while_admission_holds_the_settings_write() {
    let (client, name, _) = fixture().await;
    let control = crate::store::MongoRegistrationCapacityStore::open(&client, &name, true)
        .await
        .unwrap();
    let dispatched = Arc::new(tokio::sync::Notify::new());
    let mut options =
        mongodb::options::ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let notified = dispatched.clone();
    options.command_event_handler = Some(mongodb::event::EventHandler::callback(move |event| {
        if let mongodb::event::command::CommandEvent::Started(event) = event {
            if event.command_name == "findAndModify"
                && event.command.get_str("findAndModify").ok() == Some("platform_settings")
            {
                notified.notify_one();
            }
        }
    }));
    let operator_client = mongodb::Client::with_options(options).unwrap();
    let operator = crate::store::MongoPlatformSettingsStore::open(&operator_client, &name)
        .await
        .unwrap();
    let mut settings = operator.read().await.unwrap();
    settings.settings.limits.max_sandboxes_per_user = 0;
    let pause = Arc::new(Pause::default());
    let observed = pause.client().await;
    let candidate = crate::store::MongoRegistrationCapacityStore::open(&observed, &name, false)
        .await
        .unwrap();
    let release = pause.arm(Point::Fence);
    let admitting =
        tokio::spawn(async move { candidate.admit(&SubjectId::new("before-cutoff")).await });
    pause.wait().await;
    let mut lowering = tokio::spawn(async move {
        operator
            .replace(settings.revision, &settings.settings)
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), dispatched.notified())
        .await
        .expect("operator did not dispatch its settings write");
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(150), &mut lowering)
            .await
            .is_err(),
        "operator acknowledged while the admission transaction held its settings write"
    );
    drop(release);
    assert!(admitting.await.unwrap().unwrap());
    assert_eq!(
        lowering
            .await
            .unwrap()
            .unwrap()
            .settings
            .limits
            .max_sandboxes_per_user,
        0
    );
    assert_eq!(pause.attempts(), 1);
    assert!(control
        .existing_subject(&SubjectId::new("before-cutoff"))
        .await
        .unwrap());
    client.database(&name).drop().await.unwrap();
}
