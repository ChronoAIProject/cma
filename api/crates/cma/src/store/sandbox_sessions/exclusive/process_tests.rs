//! OS processes share only durable admission, not an AM registry or mutex.
use super::*;
use layer2_core::sandbox::store::{SandboxSessionStore, SandboxStore};
use mongodb::{bson::doc, Client, Collection};
use std::{process::Stdio, time::Duration};
use tokio::process::{Child, Command};

const CHILD: &str = "store::sandbox_sessions::exclusive::process_tests::desktop_claim_child";

#[tokio::test]
#[ignore = "invoked only by independent_processes_preserve_claim_after_holder_exit"]
async fn desktop_claim_child() {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let name = std::env::var("CMA_DESKTOP_CHILD_DATABASE").unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let coordination = client
        .database(&name)
        .collection::<Document>("desktop_test_barrier");
    let sandbox_id = layer2_core::SandboxId(std::env::var("CMA_DESKTOP_CHILD_SANDBOX").unwrap());
    let session_id = std::env::var("CMA_DESKTOP_CHILD_SESSION").unwrap();
    let sandbox = store.get_sandbox(&sandbox_id).await.unwrap().unwrap();
    coordination
        .insert_one(doc! { "_id": &session_id, "ready": true })
        .await
        .unwrap();
    wait_for(&coordination, doc! { "_id": "start" }, 1).await;
    let result = store
        .acquire_exclusive_sandbox_session(&DesktopSessionRequest {
            sandbox_id,
            session_id: session_id.clone(),
            runtime: sandbox.runtime.unwrap(),
        })
        .await;
    let result = match result {
        Ok(authority) => doc! {
            "state": "admitted",
            "authority": mongodb::bson::serialize_to_bson(&authority).unwrap(),
        },
        Err(SandboxStoreError::Conflict) => doc! { "state": "busy" },
        Err(_) => panic!("unexpected durable admission failure"),
    };
    coordination
        .update_one(doc! { "_id": session_id }, doc! { "$set": result })
        .await
        .unwrap();
    // Exiting without release models an owner process which disappears. No
    // Drop implementation or shared parent memory can free the durable claim.
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn independent_processes_preserve_claim_after_holder_exit() {
    let f = super::mongo_tests::Fixture::new().await;
    let coordination = f.db.collection::<Document>("desktop_test_barrier");
    let mut left = spawn(&f, "process-left");
    let mut right = spawn(&f, "process-right");
    assert_ne!(left.id(), right.id());
    wait_for(&coordination, doc! { "ready": true }, 2).await;
    coordination
        .insert_one(doc! { "_id": "start" })
        .await
        .unwrap();
    let (left, right) = tokio::join!(finish(&mut left), finish(&mut right));
    assert!(left.success() && right.success());
    assert_eq!(
        coordination
            .count_documents(doc! { "state": "admitted" })
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        coordination
            .count_documents(doc! { "state": "busy" })
            .await
            .unwrap(),
        1
    );
    let row = coordination
        .find_one(doc! { "state": "admitted" })
        .await
        .unwrap()
        .unwrap();
    let prior: DesktopOwnership =
        mongodb::bson::deserialize_from_bson(row["authority"].clone()).unwrap();
    assert!(matches!(
        f.right.acquire_exclusive_sandbox_session(&f.request).await,
        Err(SandboxStoreError::Conflict)
    ));

    // Wait on actual store expiry. This does not forge a clock or mutate the
    // lease to pretend that a live holder has expired.
    let successor = tokio::time::timeout(Duration::from_secs(18), async {
        loop {
            match f.right.acquire_exclusive_sandbox_session(&f.request).await {
                Ok(authority) => break authority,
                Err(SandboxStoreError::Conflict) => {
                    tokio::time::sleep(Duration::from_millis(250)).await
                }
                Err(_) => panic!("unexpected takeover failure"),
            }
        }
    })
    .await
    .unwrap();
    assert!(successor.epoch > prior.epoch);
    let old_request = DesktopSessionRequest {
        session_id: prior.session_id.clone(),
        ..f.request.clone()
    };
    assert!(matches!(
        f.left
            .renew_exclusive_sandbox_session(&old_request, &prior)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    f.left
        .release_exclusive_sandbox_session(&prior)
        .await
        .unwrap();
    assert!(f
        .right
        .renew_exclusive_sandbox_session(&f.request, &successor)
        .await
        .is_ok());
    f.right
        .release_exclusive_sandbox_session(&successor)
        .await
        .unwrap();
    f.db.drop().await.unwrap();
}

fn spawn(f: &super::mongo_tests::Fixture, session: &str) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD, "--ignored", "--nocapture"])
        .env("CMA_DESKTOP_CHILD_DATABASE", f.db.name())
        .env("CMA_DESKTOP_CHILD_SANDBOX", f.request.sandbox_id.as_str())
        .env("CMA_DESKTOP_CHILD_SESSION", session)
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()
        .unwrap()
}

async fn finish(child: &mut Child) -> std::process::ExitStatus {
    tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap()
}

async fn wait_for(collection: &Collection<Document>, filter: Document, count: u64) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if collection.count_documents(filter.clone()).await.unwrap() == count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}
