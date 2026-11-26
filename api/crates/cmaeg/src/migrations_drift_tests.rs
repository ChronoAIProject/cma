use super::*;
use mongodb::{
    options::{ClientOptions, Credential},
    Client, IndexModel,
};

async fn fixture() -> Database {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(uri).await.unwrap();
    admin.database(&format!(
        "gateway_migration_drift_{}",
        uuid::Uuid::new_v4().simple()
    ))
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn gateway_base_migration_preserves_the_channel_creation_writer_floor() {
    let db = fixture().await;
    let issuer = "https://issuer.example";
    legacy(&db, "--migrate").await.unwrap();
    crate::records::mongo::migrate_create_fence_floor(&db, issuer)
        .await
        .unwrap();
    assert!(crate::records::mongo::check_create_fence_floor(&db, issuer)
        .await
        .unwrap());

    let runner = runner(&db).await.unwrap();
    let manifest = runner.manifest(BASE).unwrap();
    assert!(cma_service_migrations::inspect(converge(&db, manifest))
        .await
        .unwrap());
    legacy(&db, "--migrate").await.unwrap();
    assert!(crate::records::mongo::check_create_fence_floor(&db, issuer)
        .await
        .unwrap());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn wrong_console_kind_refuses_adoption_using_runtime_verifier() {
    let db = fixture().await;
    use crate::console::session_backend::schema;
    schema::migrate(&db).await.unwrap();
    schema::verify(&db).await.unwrap();
    db.collection::<Document>(schema::COLLECTION)
        .update_one(doc! {"_id":"quota"}, doc! {"$set":{"kind":"unexpected"}})
        .bypass_document_validation(true)
        .await
        .unwrap();
    assert!(schema::verify(&db).await.is_err());
    let manifest = runner(&db)
        .await
        .unwrap()
        .manifest("gateway-console-sessions-v1")
        .unwrap();
    assert!(cma_service_migrations::inspect(converge(&db, manifest))
        .await
        .is_err());
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn missing_marker_does_not_mask_extra_worker_index() {
    use crate::{records::mongo, worker_credentials::mongo as worker};
    let db = fixture().await;
    mongo::migrate(&db).await.unwrap();
    worker::migrate(&db).await.unwrap();
    db.collection::<Document>(mongo::MIGRATIONS)
        .delete_one(doc! {"_id":"worker_credentials"})
        .await
        .unwrap();
    db.collection::<Document>(worker::USES)
        .create_index(IndexModel::builder().keys(doc! {"first_used_at":1}).build())
        .await
        .unwrap();
    let runner = runner(&db).await.unwrap();
    let manifest = runner.manifest("gateway-worker-credentials-v1").unwrap();
    assert!(cma_service_migrations::inspect(converge(&db, manifest))
        .await
        .is_err());
    assert!(legacy(&db, "--migrate-worker-credentials").await.is_err());
    assert!(db
        .collection::<Document>(mongo::MIGRATIONS)
        .find_one(doc! {"_id":"worker_credentials"})
        .await
        .unwrap()
        .is_none());
    let attempt = runner.status(manifest).await.unwrap();
    assert_eq!(attempt["latest_attempt"]["effects"], 0);
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn shared_startup_execution_identity_cannot_attest_drain() {
    let db = fixture().await;
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"fixture-startup","pwd":&password,"roles":[{"role":"readWrite","db":db.name()},{"role":"dbAdmin","db":db.name()}]}).await.unwrap();
    let mut options = ClientOptions::parse(std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.credential = Some(
        Credential::builder()
            .username("fixture-startup".to_owned())
            .password(password)
            .source(db.name().to_owned())
            .build(),
    );
    let startup = Client::with_options(options).unwrap().database(db.name());
    legacy(&startup, "--migrate").await.unwrap();
    let execution = runner(&startup).await.unwrap();
    assert_eq!(
        execution.activate("fixture drain").await,
        Err("designated service-local migration operator required")
    );
    let base = execution.manifest(BASE).unwrap();
    assert_eq!(
        execution
            .reconcile(base, "attempt", 1, "command", "fixture drain")
            .await,
        Err("designated service-local migration operator required")
    );
    db.run_command(cma_service_migrations::operator_role(db.name(), LEDGER))
        .await
        .unwrap();
    let operator_password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":"fixture-operator","pwd":&operator_password,"roles":[{"role":format!("{LEDGER}_operator"),"db":db.name()}]}).await.unwrap();
    let mut options = ClientOptions::parse(std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.credential = Some(
        Credential::builder()
            .username("fixture-operator".to_owned())
            .password(operator_password)
            .source(db.name().to_owned())
            .build(),
    );
    let operator = Client::with_options(options).unwrap().database(db.name());
    runner(&operator)
        .await
        .unwrap()
        .activate("fixture independently observed drain")
        .await
        .unwrap();
    assert!(legacy(&startup, "--migrate-replicas").await.is_err());
    db.drop().await.unwrap();
}

async fn validator(db: &Database, name: &str) -> Document {
    use futures_util::TryStreamExt;
    db.list_collections()
        .filter(doc! {"name":name})
        .await
        .unwrap()
        .try_next()
        .await
        .unwrap()
        .unwrap()
        .options
        .validator
        .unwrap()
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn published_optional_guard_validators_upgrade_with_retained_rows() {
    use crate::records::mongo;
    use mongodb::bson::{spec::BinarySubtype, Binary, DateTime};
    for (command, name) in [
        ("--migrate-thread-fences", mongo::THREAD_FENCES),
        ("--migrate-whatsapp-forms", mongo::WHATSAPP_FLOWS),
        ("--migrate-media-continuations", mongo::MEDIA_CONTINUATIONS),
    ] {
        let db = fixture().await;
        legacy(&db, "--migrate").await.unwrap();
        legacy(&db, command).await.unwrap();
        let target = validator(&db, name).await;
        let mut prior = target.clone();
        prior
            .get_document_mut("$jsonSchema")
            .unwrap()
            .get_document_mut("properties")
            .unwrap()
            .remove("guard_revision");
        db.run_command(doc! {"collMod":name,"validator":prior})
            .await
            .unwrap();
        let mut retained = doc! {"_id":"fixture-retained","owner":"fixture-owner","blob":Binary{subtype:BinarySubtype::Generic,bytes:b"retained".to_vec()},"version":1_i64,"schema_version":1};
        if name == mongo::MEDIA_CONTINUATIONS {
            retained.insert("expires_at", DateTime::from_millis(4_000_000_000_000));
        }
        let rows = db.collection::<Document>(name);
        rows.insert_one(retained.clone()).await.unwrap();
        legacy(&db, command).await.unwrap();
        assert_eq!(validator(&db, name).await, target);
        assert_eq!(
            rows.find_one(doc! {"_id":"fixture-retained"})
                .await
                .unwrap(),
            Some(retained.clone())
        );
        let unfamiliar = doc! {"owner":{"$type":"string"}};
        db.run_command(doc! {"collMod":name,"validator":unfamiliar.clone()})
            .await
            .unwrap();
        assert!(legacy(&db, command).await.is_err());
        assert_eq!(validator(&db, name).await, unfamiliar);
        assert_eq!(
            rows.find_one(doc! {"_id":"fixture-retained"})
                .await
                .unwrap(),
            Some(retained)
        );
        db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn media_repairs_each_missing_index_and_refuses_extra_index() {
    use crate::records::mongo;
    let db = fixture().await;
    legacy(&db, "--migrate").await.unwrap();
    legacy(&db, "--migrate-media-continuations").await.unwrap();
    let runner = runner(&db).await.unwrap();
    let manifest = runner.manifest("gateway-media-continuations-v1").unwrap();
    let rows = db.collection::<Document>(mongo::MEDIA_CONTINUATIONS);
    for index in ["owner_1__id_1", "expires_at_1"] {
        rows.drop_index(index).await.unwrap();
        assert!(!cma_service_migrations::inspect(converge(&db, manifest))
            .await
            .unwrap());
        legacy(&db, "--migrate-media-continuations").await.unwrap();
        assert!(cma_service_migrations::inspect(converge(&db, manifest))
            .await
            .unwrap());
    }
    rows.drop_index("expires_at_1").await.unwrap();
    rows.create_index(IndexModel::builder().keys(doc! {"version":1}).build())
        .await
        .unwrap();
    assert!(cma_service_migrations::inspect(converge(&db, manifest))
        .await
        .is_err());
    assert!(legacy(&db, "--migrate-media-continuations").await.is_err());
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable replica-set MongoDB fixture"]
async fn automation_v6_missing_index_recovers_in_both_entry_points() {
    let db = fixture().await;
    legacy(&db, "--migrate").await.unwrap();
    legacy(&db, "--migrate-automation").await.unwrap();
    let runner = runner(&db).await.unwrap();
    let manifest = runner.manifest("gateway-automation-v6").unwrap();
    let rows = db.collection::<Document>("eg_automation_records");
    let index = "owner_1_kind_1_next_dispatch_at_1__id_1";
    rows.drop_index(index).await.unwrap();
    assert!(!cma_service_migrations::inspect(converge(&db, manifest))
        .await
        .unwrap());
    legacy(&db, "--migrate-automation").await.unwrap();
    assert!(cma_service_migrations::inspect(converge(&db, manifest))
        .await
        .unwrap());
    assert_eq!(
        runner.status(manifest).await.unwrap()["latest_attempt"]["outcome"],
        "completed"
    );
    db.run_command(cma_service_migrations::operator_role(db.name(), LEDGER))
        .await
        .unwrap();
    let message = db.run_command(doc! {"connectionStatus":1}).await.unwrap();
    let user = message
        .get_document("authInfo")
        .unwrap()
        .get_array("authenticatedUsers")
        .unwrap()[0]
        .as_document()
        .unwrap();
    db.client().database(user.get_str("db").unwrap()).run_command(doc!{"grantRolesToUser":user.get_str("user").unwrap(),"roles":[{"role":format!("{LEDGER}_operator"),"db":db.name()}]}).await.unwrap();
    runner.activate("fixture drain").await.unwrap();
    let release = || {
        Release{source:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",image:"example.invalid/cma-event-gateway@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}
    };
    let base = runner.manifest(BASE).unwrap();
    runner
        .apply(
            base,
            &base.checksum(),
            release(),
            || cma_service_migrations::inspect(converge(&db, base)),
            || converge(&db, base),
        )
        .await
        .unwrap();
    rows.drop_index(index).await.unwrap();
    runner
        .apply(
            manifest,
            &manifest.checksum(),
            release(),
            || cma_service_migrations::inspect(converge(&db, manifest)),
            || converge(&db, manifest),
        )
        .await
        .unwrap();
    assert!(cma_service_migrations::inspect(converge(&db, manifest))
        .await
        .unwrap());
    assert_eq!(runner.status(manifest).await.unwrap()["completed"], true);
    db.drop().await.unwrap();
}
