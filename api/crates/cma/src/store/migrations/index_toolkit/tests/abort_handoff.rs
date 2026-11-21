use super::*;
use mongodb::{bson::DateTime, options::ClientOptions};
use tokio::sync::oneshot;

pub(super) async fn prepared(client: Client) -> (Database, ledger::Ledger, ledger::Lease) {
    let database = client.database(&format!("cma_abort_{}", uuid::Uuid::new_v4().simple()));
    let query = catalog::activity();
    let ledger = ledger::Ledger::for_step(client, database.clone(), query.migration().unwrap());
    ledger.initialize().await.unwrap();
    let lease = ledger.claim(&identity()).await.unwrap();
    database
        .run_command(doc! {"createIndexes":"activity","indexes":[query.index,
        {"name":"retained","key":{"other":1}}]})
        .await
        .unwrap();
    ledger
        .finish(&lease, Err("fixture lost build acknowledgement"))
        .await
        .unwrap();
    (database, ledger, lease)
}

pub(super) async fn client() -> Client {
    Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap()
}

pub(super) async fn expire(database: &Database) {
    database
        .collection::<Document>(ledger::COLLECTION)
        .update_one(
            doc! {"_id":"runner"},
            doc! {"$set":{"expires_at":DateTime::from_millis(0)}},
        )
        .await
        .unwrap();
}

pub(super) fn checkpoint() -> (
    abort::Checkpoint,
    oneshot::Receiver<()>,
    oneshot::Sender<()>,
) {
    let (reached, ready) = oneshot::channel();
    let (resume, wait) = oneshot::channel();
    ((reached, wait), ready, resume)
}

pub(super) async fn index_names(database: &Database) -> Vec<String> {
    database
        .collection::<Document>("activity")
        .list_index_names()
        .await
        .unwrap()
}

pub(super) async fn rebuild(actor: Client, database: &Database) -> Result<Value> {
    let owned_database = actor.database(database.name());
    build::execute(&catalog::activity(), actor, owned_database, &identity(), 60).await
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn paused_abort_blocks_handoff_until_its_effect_and_receipt_settle() {
    for phase in ["pending", "draining", "acknowledged"] {
        let abort_client = client().await;
        let other_client = client().await;
        let (database, ledger, predecessor) = prepared(abort_client.clone()).await;
        let (point, ready, resume) = checkpoint();
        let mut hooks = abort::Hooks::default();
        match phase {
            "pending" => hooks.pending = Some(point),
            "draining" => hooks.draining = Some(point),
            _ => hooks.acknowledged = Some(point),
        }
        let db = database.clone();
        let old_fence = predecessor.fence;
        let running = tokio::spawn(async move {
            abort::execute_inner(
                &catalog::activity(),
                abort_client,
                db,
                &identity(),
                &image(),
                old_fence,
                hooks,
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(10), ready)
            .await
            .unwrap()
            .unwrap();
        expire(&database).await;
        let status = ledger.status().await.unwrap();
        let state = if phase == "pending" {
            "pending"
        } else {
            "draining"
        };
        assert_eq!(status["destructive_ddl"]["state"], state);
        assert_eq!(status["destructive_ddl"]["recovery_required"], true);
        assert!(rebuild(other_client.clone(), &database)
            .await
            .unwrap_err()
            .starts_with("destructive index operation is unsettled"));
        assert!(ledger.finish(&predecessor, Ok(1)).await.is_err());
        // A generic failed receipt must not release the destructive barrier.
        let barrier = database
            .collection::<Document>(ledger::COLLECTION)
            .find_one(doc! {"_id":"destructive_ddl"})
            .await
            .unwrap()
            .unwrap();
        let abort_lease = ledger::Lease {
            attempt: barrier.get_str("attempt").unwrap().into(),
            fence: barrier.get_i64("fence").unwrap(),
        };
        assert!(ledger
            .finish(&abort_lease, Err("fixture failure is not settlement"))
            .await
            .is_err());
        assert!(!ledger.completed().await.unwrap());
        resume.send(()).unwrap();
        assert_eq!(
            running.await.unwrap().unwrap()["destructive_state"],
            "settled"
        );
        assert_eq!(
            ledger.status().await.unwrap()["destructive_ddl"]["state"],
            "settled"
        );
        assert_eq!(index_names(&database).await, vec!["_id_", "retained"]);
        assert_eq!(
            rebuild(other_client.clone(), &database).await.unwrap()["created_indexes"],
            1
        );
        assert!(ledger.completed().await.unwrap());
        assert!(ledger.finish(&predecessor, Ok(1)).await.is_err());
        assert!(ledger.begin_abort(&abort_lease).await.is_err());
        assert!(abort::execute(
            &catalog::activity(),
            other_client.clone(),
            other_client.database(database.name()),
            &identity(),
            &image(),
            old_fence
        )
        .await
        .is_err());
        let indexes = index_names(&database).await;
        assert!(indexes
            .iter()
            .any(|name| name == catalog::activity().index_name()));
        assert!(indexes.iter().any(|name| name == "retained"));
        database.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB with test commands"]
async fn unknown_drop_keeps_barrier_after_expiry_and_visible_server_effect() {
    let admin = client().await;
    let app = format!("abort-unknown-{}", uuid::Uuid::new_v4().simple());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.app_name = Some(app.clone());
    let abort_client = Client::with_options(options).unwrap();
    let (database, ledger, predecessor) = prepared(abort_client.clone()).await;
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":1},
        "data":{"failCommands":["dropIndexes"],"appName":app,
        "writeConcernError":{"code":64,"codeName":"WriteConcernFailed","errmsg":"fixture uncertain drop acknowledgement"}}}).await.unwrap();
    let result = abort::execute(
        &catalog::activity(),
        abort_client,
        database.clone(),
        &identity(),
        &image(),
        predecessor.fence,
    )
    .await;
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await
        .unwrap();
    assert!(result.is_err());
    // A majority write makes the earlier successful DDL visible despite the
    // injected error bypassing its normal acknowledgement wait.
    database.run_command(doc! {"insert":"visibility","documents":[{"_id":"barrier"}],"writeConcern":{"w":"majority"}}).await.unwrap();
    assert_eq!(index_names(&database).await, vec!["_id_", "retained"]);
    expire(&database).await;
    assert_eq!(
        ledger.status().await.unwrap()["destructive_ddl"]["state"],
        "draining"
    );
    assert!(rebuild(admin, &database)
        .await
        .unwrap_err()
        .starts_with("destructive index operation is unsettled"));
    assert!(!ledger.completed().await.unwrap());
    assert!(ledger.finish(&predecessor, Ok(1)).await.is_err());
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn cancelled_abort_continuation_does_not_release_its_barrier() {
    let abort_client = client().await;
    let (database, ledger, predecessor) = prepared(abort_client.clone()).await;
    let (point, ready, _resume) = checkpoint();
    let db = database.clone();
    let running = tokio::spawn(async move {
        abort::execute_inner(
            &catalog::activity(),
            abort_client,
            db,
            &identity(),
            &image(),
            predecessor.fence,
            abort::Hooks {
                draining: Some(point),
                ..Default::default()
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(10), ready)
        .await
        .unwrap()
        .unwrap();
    running.abort();
    assert!(running.await.unwrap_err().is_cancelled());
    expire(&database).await;
    assert_eq!(
        ledger.status().await.unwrap()["destructive_ddl"]["state"],
        "draining"
    );
    assert!(rebuild(client().await, &database)
        .await
        .unwrap_err()
        .starts_with("destructive index operation is unsettled"));
    assert!(index_names(&database)
        .await
        .iter()
        .any(|name| name == catalog::activity().index_name()));
    database.drop().await.unwrap();
}
