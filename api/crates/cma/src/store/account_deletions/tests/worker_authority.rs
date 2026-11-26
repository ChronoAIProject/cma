use super::worker_fixture::*;
use mongodb::{
    options::{ClientOptions, Credential},
    Client,
};

async fn runtime(db: &Database, legacy: bool) -> Database {
    let name = if legacy {
        "legacy-runtime"
    } else {
        "journal-runtime"
    };
    let mut role = if legacy {
        crate::store::isolation::role(db.name())
    } else {
        worker_schema::runtime_role(db.name())
    };
    role.insert("createRole", name);
    if legacy {
        role.get_array_mut("privileges").unwrap().retain(|grant| {
            let collection = grant
                .as_document()
                .unwrap()
                .get_document("resource")
                .unwrap()
                .get_str("collection")
                .unwrap();
            !worker_schema::COLLECTIONS.contains(&collection)
                && collection != "agent_manager_migrations"
        });
    }
    db.run_command(role).await.unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    db.run_command(doc! {"createUser":name,"pwd":&password,"roles":[{"role":name,"db":db.name()}]})
        .await
        .unwrap();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.credential = Some(
        Credential::builder()
            .username(name.to_owned())
            .password(password)
            .source(db.name().to_owned())
            .build(),
    );
    let client = Client::with_options(options).unwrap();
    let database = client.database(db.name());
    crate::store::isolation::verify(&database).await.unwrap();
    database
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn legacy_role_keeps_non_enrolled_writes_and_journal_role_cannot_index_or_delete() {
    let (_, db, admin) = fixture().await;
    let legacy = runtime(&db, true).await;
    let modern = runtime(&db, false).await;
    let at = database_now(&db).await;
    let accepted = admin
        .request(&SubjectId::new("owner"), "accepted", &at)
        .await
        .unwrap();
    let generic = MongoAccountDeletionStore::from_database(&legacy);
    let held = generic
        .claim_due("legacy", &at, Duration::from_secs(90))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(held.id, accepted.id);
    assert!(!db
        .list_collection_names()
        .await
        .unwrap()
        .contains(&worker_schema::WORK.into()));
    let journal = MongoErasureJournal::from_database(&modern);
    let request = JournalRequest::new(binding(accepted.id), JournalCommand::Enroll).unwrap();
    assert!(matches!(
        journal.enroll(&request, &budget()).await,
        Err(JournalError::Unprepared)
    ));
    crate::store::migrations::account_erasure_work::prepare_for_test(&db).await;
    journal.enroll(&request, &budget()).await.unwrap();
    let mut changed = held.clone();
    changed.revision += 1;
    let before = snapshot(&db).await;
    assert!(generic
        .commit(
            changed,
            vec![],
            held.revision,
            Some(&held.work.lease.as_ref().unwrap().token)
        )
        .await
        .is_err());
    assert_eq!(snapshot(&db).await, before);
    for name in worker_schema::COLLECTIONS {
        assert!(modern
            .collection::<Document>(name)
            .delete_many(doc! {})
            .await
            .is_err());
        assert!(modern
            .run_command(
                doc! {"createIndexes":*name,"indexes":[{"key":{"other":1},"name":"unreviewed"}]}
            )
            .await
            .is_err());
    }
    db.drop().await.unwrap();
}
