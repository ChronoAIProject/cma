use super::*;
use crate::sandbox::{CredentialCustodyError, CredentialIssuance, IssuanceIdentity, IssuanceState};
use mongodb::event::{command::CommandEvent, EventHandler};
use std::sync::{atomic::AtomicBool, atomic::Ordering, mpsc, Mutex};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn issuance_read_keeps_marker_and_intent_at_one_snapshot() {
    let f = Fixture::new("issuance_read_snapshot").await;
    let observed = Arc::new(tokio::sync::Notify::new());
    let armed = Arc::new(AtomicBool::new(false));
    let (resume, released) = mpsc::sync_channel(1);
    let released = Mutex::new(released);
    let ready = observed.clone();
    let gate = armed.clone();
    let namespace = format!("{}.sandbox_credential_issuance", f.name);
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        let CommandEvent::Succeeded(event) = event else {
            return;
        };
        if event.command_name == "find"
            && event.reply.get_document("cursor").is_ok_and(|cursor| {
                cursor.get_str("ns").ok() == Some(namespace.as_str())
                    && cursor.get_array("firstBatch").is_ok_and(Vec::is_empty)
            })
            && gate.swap(false, Ordering::SeqCst)
        {
            // Hold the completed first read while an independent client commits
            // issuance. The callback is synchronous; the other runtime worker
            // drives the writer without relying on a scheduling delay.
            ready.notify_one();
            released
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .expect("issuance writer did not release the reader");
        }
    }));
    let reader = Arc::new(
        MongoSandboxCredentialStore::open(
            &Client::with_options(options).unwrap(),
            &f.name,
            "work-contract",
            &f.key,
        )
        .await
        .unwrap(),
    );
    armed.store(true, Ordering::SeqCst);
    let reading = {
        let reader = reader.clone();
        let claim = f.claim.clone();
        tokio::spawn(async move { reader.issuance(&claim).await })
    };
    tokio::time::timeout(Duration::from_secs(5), observed.notified())
        .await
        .expect("reader did not reach the empty intent response");
    let lease = f.claim.work.lease.as_ref().unwrap();
    let intent = CredentialIssuance {
        operation_id: format!("mint_{}", uuid::Uuid::new_v4().simple()),
        sandbox_id: f.claim.id.clone(),
        owner: f.claim.owner.clone(),
        generation: f.claim.credential.next_generation().unwrap(),
        identity: IssuanceIdentity::Prepared {
            provider_identity: "a".repeat(64),
            request_digest: "b".repeat(64),
        },
        owner_authority: None,
        original_holder: lease.holder.clone(),
        original_fence: lease.fence,
        created_at: at(0),
        next_check_at: at(0),
        state: IssuanceState::Unknown,
    };
    assert!(f.custody.begin_issuance(&f.claim, &intent).await.unwrap());
    assert_eq!(
        f.custody.issuance(&f.claim).await.unwrap(),
        Some(intent.clone())
    );
    resume.send(()).unwrap();
    let before_commit = reading.await.unwrap();
    let after_commit = reader.issuance(&f.claim).await.unwrap();
    f.client
        .database(&f.name)
        .collection::<Document>("sandbox_credential_issuance")
        .delete_one(doc! {"_id":f.claim.id.as_str()})
        .write_concern(mongodb::options::WriteConcern::majority())
        .await
        .unwrap();
    let missing = reader.issuance(&f.claim).await;
    f.cleanup().await;
    assert_eq!(before_commit.unwrap(), None);
    assert_eq!(after_commit, Some(intent));
    assert!(matches!(
        missing,
        Err(CredentialCustodyError::Unavailable(_))
    ));
}
