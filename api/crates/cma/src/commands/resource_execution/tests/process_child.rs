use super::super::sweep::{RevocationSweep, SweepEvent};
use super::process_fixture::*;
use super::*;
use crate::store::MongoSandboxStore;
use mongodb::{bson::doc, Client};

#[tokio::test]
#[ignore = "invoked only by worker_process_death_recovers_original_revocation"]
async fn revocation_worker_child() {
    let name = std::env::var("CMA_REVOCATION_CHILD_DATABASE").unwrap();
    assert!(name.starts_with("cma_revocation_worker_"));
    let pause = match std::env::var("CMA_REVOCATION_CHILD_MODE").as_deref() {
        Ok("pause") => true,
        Ok("recover") => false,
        _ => panic!("explicit child mode required"),
    };
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let collection = client.database(&name).collection::<Script>(COLLECTION);
    let script = collection
        .find_one(doc! {"_id": "participant"})
        .await
        .unwrap()
        .unwrap();
    let original = script.request.revocation.operation_id.clone();
    let directory = Arc::new(ProcessDirectory(Arc::new(Receiver {
        script,
        collection,
        pause,
    })));
    let store = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .open_resource_execution()
        .await
        .unwrap();
    let mut sweep = RevocationSweep::new(Arc::new(RevocationWorker::new(store, directory)));
    let mut discovered = false;
    loop {
        match sweep.next().await {
            SweepEvent::Reconciled { operation, result } => {
                assert_eq!(operation, original);
                assert_eq!(result.unwrap(), Progress::Effective);
                break;
            }
            SweepEvent::DiscoveryFailed(error) => panic!("child discovery failed: {error}"),
            SweepEvent::Discovered { count, end_of_scan } if !discovered => {
                assert_eq!(count, 1, "original pending operation missing");
                assert!(end_of_scan);
                discovered = true;
            }
            _ => {}
        }
    }
    assert!(sweep.shutdown().await.is_empty());
    println!("{COMPLETED}");
}
