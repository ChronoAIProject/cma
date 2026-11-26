use super::super::{role, verify_with_authority};
use cma_service_auth::mongodb_authority::MongoAuthorityMode;
use mongodb::{
    bson::{doc, Bson, Document},
    error::ErrorKind,
    options::{ClientOptions, Credential, Hint, ReadConcern, WriteConcern},
    Client, Database,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opening {
    Collectionless,
    IndexedRead,
}

async fn probe(runtime: &Database, mode: MongoAuthorityMode, opening: Opening) {
    let mut session = runtime.client().start_session().await.unwrap();
    session
        .start_transaction()
        .read_concern(ReadConcern::snapshot())
        .write_concern(WriteConcern::majority())
        .await
        .unwrap();

    let result = match opening {
        Opening::Collectionless => runtime
            .run_command(doc! {
                "aggregate": 1_i32,
                "pipeline": [{ "$documents": [doc! {}] }],
                "cursor": {},
            })
            .session(&mut session)
            .await
            .map(|result| {
                let cursor = result.get_document("cursor").unwrap();
                assert_eq!(cursor.get_i64("id").unwrap(), 0);
                let rows = cursor.get_array("firstBatch").unwrap();
                assert_eq!(rows, &[Bson::Document(doc! {})]);
                rows.len()
            }),
        Opening::IndexedRead => runtime
            .collection::<Document>("agents")
            .find_one(doc! { "_id": Bson::MinKey })
            .hint(Hint::Keys(doc! { "_id": 1_i32 }))
            .session(&mut session)
            .await
            .map(|row| {
                assert!(row.is_none());
                0
            }),
    };

    match result {
        Ok(rows) => {
            session.commit_transaction().await.unwrap();
            eprintln!(
                "{mode:?} {opening:?} authorization=accepted rows={rows} transaction=committed"
            );
        }
        Err(error) => {
            session.abort_transaction().await.unwrap();
            assert_eq!(opening, Opening::Collectionless, "{error}");
            let ErrorKind::Command(command) = error.kind.as_ref() else {
                panic!("collectionless read did not return an authorization result: {error}");
            };
            assert_eq!(command.code, 13, "{error}");
            assert_eq!(command.code_name, "Unauthorized");
            eprintln!(
                "{mode:?} {opening:?} authorization=denied code=13 rows=absent transaction=aborted"
            );
        }
    }
}

async fn barrier_allows_new_agent_index_and_independent_sandbox_write(
    admin: &Database,
    runtime: &Client,
    writer: &Client,
    index_name: &str,
) {
    let mut held = runtime.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut held)
        .await
        .unwrap();

    let rows = writer
        .database(admin.name())
        .collection::<Document>("sandboxes");
    let (index, written) = tokio::join!(
        admin.run_command(doc! {
            "createIndexes": "agents",
            "indexes": [{"key": {index_name: 1_i32}, "name": index_name}],
            "maxTimeMS": 2_000_i32,
        }),
        async {
            let mut session = writer.start_session().await.unwrap();
            crate::store::sandbox_mongo::start_transaction(&mut session)
                .await
                .unwrap();
            let result = rows
                .update_one(
                    doc! {"_id": "synthetic-sandbox"},
                    doc! {"$inc": {"writes": 1}},
                )
                .session(&mut session)
                .await
                .unwrap();
            session.commit_transaction().await.unwrap();
            result
        },
    );
    let index = index.unwrap();
    assert_eq!(
        index.get_i32("numIndexesAfter").unwrap(),
        index.get_i32("numIndexesBefore").unwrap() + 1
    );
    assert_eq!(written.modified_count, 1);
    held.commit_transaction().await.unwrap();
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo fixture"]
async fn collectionless_and_indexed_transaction_opening_under_runtime_grants() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let build = admin
        .database("admin")
        .run_command(doc! { "buildInfo": 1_i32 })
        .await
        .unwrap();
    eprintln!(
        "MongoDB server version {}",
        build.get_str("version").unwrap()
    );
    let name = format!("cma_txn_open_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    db.run_command(role(&name)).await.unwrap();
    let agents = db.collection::<Document>("agents");
    let sentinel = doc! { "_id": "synthetic-agent" };
    agents.insert_one(sentinel.clone()).await.unwrap();
    let sandboxes = db.collection::<Document>("sandboxes");
    sandboxes
        .insert_one(doc! {"_id": "synthetic-sandbox", "writes": 0})
        .await
        .unwrap();

    for (mode, username, roles) in [
        (
            MongoAuthorityMode::Isolated,
            "isolated-runtime",
            vec![doc! { "role": "cma_am_runtime", "db": &name }],
        ),
        (
            MongoAuthorityMode::SharedSchema,
            "shared-runtime",
            vec![
                doc! { "role": "readWrite", "db": &name },
                doc! { "role": "dbAdmin", "db": &name },
            ],
        ),
    ] {
        let password = uuid::Uuid::new_v4().to_string();
        db.run_command(doc! { "createUser": username, "pwd": &password, "roles": roles })
            .await
            .unwrap();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.credential = Some(
            Credential::builder()
                .username(username.to_owned())
                .password(password)
                .source(name.clone())
                .build(),
        );
        crate::store::transaction_barrier::bind(&mut options, &name);
        let writer = Client::with_options(options.clone()).unwrap();
        let runtime = Client::with_options(options).unwrap();
        let database = runtime.database(&name);
        verify_with_authority(&database, mode).await.unwrap();
        probe(&database, mode, Opening::Collectionless).await;
        probe(&database, mode, Opening::IndexedRead).await;
        barrier_allows_new_agent_index_and_independent_sandbox_write(
            &db, &runtime, &writer, username,
        )
        .await;
    }
    assert_eq!(agents.find_one(doc! {}).await.unwrap(), Some(sentinel));
    assert_eq!(agents.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        sandboxes
            .find_one(doc! {"_id": "synthetic-sandbox"})
            .await
            .unwrap(),
        Some(doc! {"_id": "synthetic-sandbox", "writes": 2})
    );
    db.drop().await.unwrap();
}
