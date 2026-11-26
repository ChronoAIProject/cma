//! Lost acknowledgements must reconcile durable state without repeating intent.
//! The owner runs this with the other Mongo tests, serially, on its disposable
//! replica set with enableTestCommands. Failpoints match this client's appName.
use super::*;
use futures_util::FutureExt;
use mongodb::{
    bson::{doc, Document},
    options::ClientOptions,
    Client,
};
use std::time::Duration;

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set with failpoints"]
async fn uncertain_create_and_delete_reconcile_without_reinsertion() {
    let uri = std::env::var("CMA_AUTHENTICATED_TEST_MONGODB_URI")
        .expect("disposable Mongo fixture required");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let name = format!("cma_console_fault_{}", uuid::Uuid::new_v4().simple());
    let db = admin.database(&name);
    schema::migrate(&db).await.unwrap();
    let application = format!("console-fault-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(application.clone());
    options.server_selection_timeout = Some(Duration::from_secs(2));
    let client = Client::with_options(options).unwrap();
    let backend = MongoSessionBackend::new(client.database(&name));
    let result = std::panic::AssertUnwindSafe(async {
        let sid = format!("{:064x}", uuid::Uuid::new_v4().as_u128());
        let subject = format!("{:064x}", uuid::Uuid::new_v4().as_u128());
        let limits = SessionLimits {
            ttl: Duration::from_secs(600),
            idle: Duration::from_secs(60),
            max: 10,
            per_subject: 3,
        };
        fail_ack(&admin, &application, "commitTransaction").await;
        let created = backend
            .create(&sid, &subject, vec![71], 90_000, &limits)
            .await
            .unwrap();
        clear(&admin).await;
        assert_eq!(created.payload, [71]);
        let rows = db.collection::<Document>(schema::COLLECTION);
        let quota = rows
            .find_one(doc! {"_id":schema::QUOTA})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            quota.get_i64("fence").unwrap(),
            1,
            "uncertain admission was not repeated"
        );
        assert_eq!(
            rows.count_documents(doc! {"kind":"session"}).await.unwrap(),
            1
        );

        fail_ack(&admin, &application, "findAndModify").await;
        assert_eq!(
            backend.revoke(&sid).await.unwrap(),
            None,
            "lost deletion receipt reconciles absence"
        );
        clear(&admin).await;
        assert!(backend.load(&sid, false).await.unwrap().is_none());
        assert!(backend
            .compare_exchange(&sid, &[71], vec![72], None)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            rows.count_documents(doc! {"kind":"session"}).await.unwrap(),
            0
        );
    })
    .catch_unwind()
    .await;
    clear(&admin).await;
    db.drop().await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

async fn fail_ack(admin: &Client, application: &str, command: &str) {
    admin.database("admin").run_command(doc! {
        "configureFailPoint":"failCommand","mode":"alwaysOn","data":{
            "appName":application,"failCommands":[command],
            "writeConcernError":{"code":64,"codeName":"WriteConcernFailed","errmsg":"controlled acknowledgement loss"},
            "errorLabels":["UnknownTransactionCommitResult"]
        }
    }).await.expect("owned fixture must support acknowledgement-loss failpoint");
}

async fn clear(admin: &Client) {
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .expect("owned fixture failpoint cleanup");
}
