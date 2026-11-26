//! Real authenticated role proof through the canonical segment migrator.
use super::*;
use crate::store::{
    agent_capacity_recovery::records,
    agent_log_segments::{
        self as segments, publication, transaction_scope::TransactionScope, writer_admission,
        writer_gate,
    },
    isolation,
    sandbox_mongo::{start_transaction, MongoSandboxStore},
};
use mongodb::options::{ClientOptions, Credential};

async fn principal(uri: &str, db: &mongodb::Database, role: Document, user: &str) -> Client {
    let name = role.get_str("createRole").unwrap().to_owned();
    db.run_command(role).await.unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":user,"pwd":&password,
    "roles":[{"role":name,"db":db.name()}]})
        .await
        .unwrap();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.credential = Some(
        Credential::builder()
            .username(user.to_owned())
            .password(password)
            .source(db.name().to_owned())
            .build(),
    );
    Client::with_options(options).unwrap()
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn canonical_migrator_and_isolated_runtime_cover_controls_and_format2_work() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_seg_role_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    let migrator = principal(
        &uri,
        &db,
        authority::role_for(&name, true, Step::AgentLogSegments),
        "migrator",
    )
    .await;
    let schema_db = migrator.database(&name);
    let privileges = schema_db
        .run_command(doc! {"connectionStatus":1,"showPrivileges":true})
        .await
        .unwrap();
    authority::verify_for(
        &privileges,
        &name,
        MongoAuthorityMode::Isolated,
        true,
        Step::AgentLogSegments,
    )
    .unwrap();
    let ledger = ledger::Ledger::for_step(migrator, schema_db.clone(), Step::AgentLogSegments);
    let plan = agent_log_segments::inspect(&schema_db).await.unwrap();
    assert_eq!(plan["writes"], false);
    let result = agent_log_segments::apply(&ledger, &schema_db, &identity())
        .await
        .unwrap();
    assert_eq!(result["outcome"], "completed");
    assert_eq!(
        segments::schema::pending_count(&schema_db).await.unwrap(),
        0
    );
    let runtime = principal(&uri, &db, isolation::role(&name), "runtime").await;
    let runtime_db = runtime.database(&name);
    isolation::verify(&runtime_db).await.unwrap();
    let store = MongoSandboxStore::open(&runtime, &name).await.unwrap();
    assert!(store.capacity_recovery_work_exists().await.unwrap());
    let agent = records::test_support::agent();
    let operation = publication::PublicationIdentity {
        key: "role-contract".into(),
        appended_at: records::test_support::NOW.into(),
        fingerprint: publication::result_digest(&"original-role-contract").unwrap(),
    };
    let mut session = runtime.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    writer_gate::replace(
        &runtime_db,
        writer_gate::GateName::Segments,
        0,
        writer_gate::Policy {
            enabled: true,
            agents: vec![agent.id.to_string()],
            ..Default::default()
        },
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let admitted = writer_admission::admit(
        &runtime_db,
        &agent,
        &operation,
        writer_admission::AdmissionKind::Activation,
        records::test_support::NOW,
        &mut session,
    )
    .await
    .unwrap();
    session.commit_transaction().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    assert_eq!(
        writer_admission::read(&runtime_db, &agent, &operation, &mut session)
            .await
            .unwrap()
            .unwrap()
            .id,
        admitted.id
    );
    session.abort_transaction().await.unwrap();
    let work = records::test_support::stored();
    let key = work.work.request.key().unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    records::insert(&store, &work, &mut scope).await.unwrap();
    scope.release().commit_transaction().await.unwrap();
    let mut scope = TransactionScope::start(&mut session).await.unwrap();
    assert_eq!(
        records::read(&store, &key, &mut scope).await.unwrap(),
        Some(work)
    );
    scope.release().abort_transaction().await.unwrap();
    let carriers = runtime_db.collection::<Document>(records::COLLECTION);
    assert_eq!(
        carriers
            .update_one(
                doc! {"_id":&key},
                doc! {"$set":{"work.ready_at":records::test_support::NOW}}
            )
            .await
            .unwrap()
            .matched_count,
        1
    );
    let original = carriers.find_one(doc! {"_id":&key}).await.unwrap().unwrap();
    assert_eq!(original.get_i64("format_version").unwrap(), 2);
    for missing_witness in [false, true] {
        let mut invalid = original.clone();
        if missing_witness {
            invalid.remove("result");
        } else {
            invalid.insert("format_version", 1_i64);
        }
        let error = carriers
            .replace_one(doc! {"_id":&key}, invalid)
            .await
            .unwrap_err();
        assert!(
            matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Write(
            mongodb::error::WriteFailure::WriteError(error)) if error.code == 121)
        );
        assert_eq!(
            carriers.find_one(doc! {"_id":&key}).await.unwrap().unwrap(),
            original
        );
    }
    for foreign in [
        "eg_channel_records",
        "im_images",
        "agent_capacity_recovery_work_v1",
    ] {
        let error = runtime_db
            .collection::<Document>(foreign)
            .find_one(doc! {})
            .await
            .unwrap_err();
        assert!(
            matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code == 13)
        );
    }
    for (index, collection) in [
        writer_gate::COLLECTION,
        writer_admission::COLLECTION,
        records::COLLECTION,
    ]
    .into_iter()
    .enumerate()
    {
        let mut role = isolation::role(&name);
        role.insert("createRole", format!("missing-sidecar-{index}"));
        role.get_array_mut("privileges").unwrap().retain(|grant| {
            grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap()
                != collection
        });
        let restricted = principal(&uri, &db, role, &format!("missing-{index}")).await;
        assert!(isolation::verify(&restricted.database(&name))
            .await
            .is_err());
        let error = restricted
            .database(&name)
            .collection::<Document>(collection)
            .find_one(doc! {})
            .await
            .unwrap_err();
        assert!(
            matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(c) if c.code == 13)
        );
    }
    db.drop().await.unwrap();
}
