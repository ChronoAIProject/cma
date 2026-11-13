//! Command budgets are observed at the driver boundary, without logging bodies.
use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Commands {
    names: Vec<String>,
    transaction: Option<(Document, i64)>,
    starts: usize,
    same_transaction: bool,
}

impl Commands {
    fn take(&mut self) -> Self {
        std::mem::replace(
            self,
            Self {
                same_transaction: true,
                ..Self::default()
            },
        )
    }

    fn assert(&self, expected: &[&str]) {
        assert_eq!(self.names, expected);
        assert_eq!(self.starts, 1);
        assert!(
            self.same_transaction,
            "every operation must reuse the transaction"
        );
    }
}

async fn observed() -> (Fixture, Arc<Mutex<Commands>>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let commands = Arc::new(Mutex::new(Commands::default()));
    let events = commands.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let mut commands = events.lock().unwrap();
            commands.names.push(event.command_name);
            commands.starts +=
                usize::from(event.command.get_bool("startTransaction").ok() == Some(true));
            let transaction = event
                .command
                .get_document("lsid")
                .ok()
                .zip(event.command.get_i64("txnNumber").ok())
                .map(|(session, transaction)| (session.clone(), transaction));
            if commands.transaction.is_none() {
                commands.transaction.clone_from(&transaction);
            }
            commands.same_transaction &= transaction.is_some()
                && transaction == commands.transaction
                && event.command.get_bool("autocommit").ok() == Some(false);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let peer = Client::with_uri_str(&uri).await.unwrap();
    let fixture = Fixture::with_clients(client, peer).await;
    commands.lock().unwrap().take();
    (fixture, commands)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn empty_and_released_claims_skip_reclamation_with_one_transaction() {
    let (f, commands) = observed().await;
    for _ in 0..2 {
        let owner = f
            .left
            .acquire_exclusive_sandbox_session(&f.request)
            .await
            .unwrap();
        commands.lock().unwrap().take().assert(&[
            "findAndModify",
            "find",
            "insert",
            "commitTransaction",
        ]);
        let renewed = f
            .left
            .renew_exclusive_sandbox_session(&f.request, &owner)
            .await
            .unwrap();
        commands
            .lock()
            .unwrap()
            .take()
            .assert(&["findAndModify", "update", "commitTransaction"]);
        f.left
            .release_exclusive_sandbox_session(&renewed)
            .await
            .unwrap();
        commands.lock().unwrap().take();
    }
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn indexed_latest_session_preserves_legacy_exclusion_and_expired_reclamation() {
    let (f, commands) = observed().await;
    for (id, expiry) in [
        ("expired-a", "2000-01-01T00:00:00Z"),
        ("legacy-live", "9999-01-01T00:00:00Z"),
        ("expired-b", "2001-01-01T00:00:00Z"),
    ] {
        f.right
            .open_sandbox_session(&f.request.sandbox_id, id, expiry)
            .await
            .unwrap();
    }
    assert!(matches!(
        f.left.acquire_exclusive_sandbox_session(&f.request).await,
        Err(SandboxStoreError::Conflict)
    ));
    commands
        .lock()
        .unwrap()
        .take()
        .assert(&["findAndModify", "find", "abortTransaction"]);
    assert_eq!(
        f.db.collection::<Document>("sandbox_sessions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        3
    );
    f.right
        .close_sandbox_session(&f.request.sandbox_id, "legacy-live")
        .await
        .unwrap();
    commands.lock().unwrap().take();
    let owner = f
        .left
        .acquire_exclusive_sandbox_session(&f.request)
        .await
        .unwrap();
    commands.lock().unwrap().take().assert(&[
        "findAndModify",
        "find",
        "delete",
        "insert",
        "commitTransaction",
    ]);
    assert_eq!(
        f.db.collection::<Document>("sandbox_sessions")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.right
        .release_exclusive_sandbox_session(&owner)
        .await
        .unwrap();
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn stale_epoch_refuses_at_parent_write_without_touching_session() {
    let (f, commands) = observed().await;
    let owner = f
        .left
        .acquire_exclusive_sandbox_session(&f.request)
        .await
        .unwrap();
    let mut stale = owner.clone();
    stale.epoch += 1;
    commands.lock().unwrap().take();
    assert!(matches!(
        f.left
            .renew_exclusive_sandbox_session(&f.request, &stale)
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    commands
        .lock()
        .unwrap()
        .take()
        .assert(&["findAndModify", "abortTransaction"]);
    // Failure rolls back the maintenance fence and leaves the real owner usable.
    f.right
        .renew_exclusive_sandbox_session(&f.request, &owner)
        .await
        .unwrap();
    f.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI disposable replica set"]
async fn malformed_latest_lease_refuses_without_reclamation_or_epoch_change() {
    let (f, commands) = observed().await;
    for expiry in [mongodb::bson::Bson::Int32(42), "invalid".into()] {
        f.right
            .sandbox_sessions
            .insert_one(doc! {
                "_id": "malformed", "sandbox_id": f.request.sandbox_id.as_str(),
                "lease_expires_at": expiry,
            })
            .await
            .unwrap();
        commands.lock().unwrap().take();
        assert!(matches!(
            f.left.acquire_exclusive_sandbox_session(&f.request).await,
            Err(SandboxStoreError::Unavailable(_))
        ));
        commands
            .lock()
            .unwrap()
            .take()
            .assert(&["findAndModify", "find", "abortTransaction"]);
        assert_eq!(
            f.right
                .sandbox_sessions
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        f.right
            .sandbox_sessions
            .delete_one(doc! {"_id": "malformed"})
            .await
            .unwrap();
    }
    let owner = f
        .right
        .acquire_exclusive_sandbox_session(&f.request)
        .await
        .unwrap();
    assert_eq!(
        owner.epoch, 1,
        "refused transactions must not advance the fence"
    );
    f.db.drop().await.unwrap();
}
