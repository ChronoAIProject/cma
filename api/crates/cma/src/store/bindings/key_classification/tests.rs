use super::*;
use crate::store::bindings::mongo_tests::{binding, fixture, NOW};
use layer2_core::sandbox::binding::{BindingState, BindingWork};
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
};
use std::sync::{Arc, Mutex};

async fn ready(store: &MongoSandboxStore, mut value: ProviderBinding) -> ProviderBinding {
    let revision = value.revision;
    value.state = BindingState::Ready;
    value.work = BindingWork::Idle;
    value.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(revision), value.clone())
        .await
        .unwrap();
    value
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn rotation_between_unique_barrier_and_history_keeps_all_retired_hashes() {
    let (writer, database, store) = fixture().await;
    let observed = Arc::new(Mutex::new(Vec::<Document>::new()));
    let capture = observed.clone();
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.command_name == "find" {
                capture.lock().unwrap().push(event.command);
            }
        }
    }));
    let reader = mongodb::Client::with_options(options).unwrap();
    let lookup = MongoSandboxStore::open(&reader, &database).await.unwrap();
    let original = binding();
    store.save_binding(None, original.clone()).await.unwrap();
    let original = ready(&store, original).await;
    let first = original
        .rotate_credential("b".repeat(64), NOW.into())
        .unwrap();
    store
        .save_binding(Some(original.revision), first.clone())
        .await
        .unwrap();
    observed.lock().unwrap().clear();
    let mut missing = reader
        .start_session()
        .causal_consistency(true)
        .await
        .unwrap();
    assert!(matches!(
        history(&lookup, &original.key_sha256, &mut missing).await,
        Err(SandboxStoreError::Unavailable(_))
    ));
    assert!(observed.lock().unwrap().is_empty());
    let CurrentOrBarrier::History(mut barrier) = current_or_barrier(&lookup, &original.key_sha256)
        .await
        .unwrap()
    else {
        panic!("rotated original must enter retained history lookup");
    };
    let barrier_time = barrier
        .operation_time()
        .expect("linearizable response advances operationTime");
    // Another process rotates and retires the binding after the barrier. Its
    // append-only former-key history must remain recognized in the later read.
    let first = ready(&store, first).await;
    let second = first.rotate_credential("c".repeat(64), NOW.into()).unwrap();
    store
        .save_binding(Some(first.revision), second.clone())
        .await
        .unwrap();
    let mut retired = second.clone();
    retired.state = BindingState::Retired;
    retired.work = BindingWork::RetireCurrentConversations;
    retired.advance_revision(NOW.into()).unwrap();
    store
        .save_binding(Some(second.revision), retired)
        .await
        .unwrap();
    assert!(history(&lookup, &original.key_sha256, &mut barrier)
        .await
        .unwrap());
    {
        let commands = observed.lock().unwrap();
        assert_eq!(commands.len(), 2);
        assert_eq!(
            commands[0].get_document("filter").unwrap(),
            &doc! {"record.key_sha256": &original.key_sha256}
        );
        let initial = commands[0].get_document("readConcern").unwrap();
        assert_eq!(initial.get_str("level").unwrap(), "linearizable");
        assert!(!initial.contains_key("afterClusterTime"));
        assert_eq!(
            commands[1].get_document("filter").unwrap(),
            &doc! {"key_history": &original.key_sha256}
        );
        let later = commands[1].get_document("readConcern").unwrap();
        assert_eq!(later.get_str("level").unwrap(), "majority");
        assert_eq!(
            later.get_timestamp("afterClusterTime").unwrap(),
            barrier_time
        );
        assert_eq!(commands[0].get("lsid"), commands[1].get("lsid"));
        for command in commands.iter() {
            // The driver encodes this small timeout as BSON Int32. Keep the
            // assertion about the exact 750 ms budget without requiring an
            // implementation-specific BSON integer width.
            assert_eq!(command.get_i32("maxTimeMS").unwrap(), 750);
            assert!(!command.contains_key("txnNumber"));
        }
    }
    for digest in [&original.key_sha256, &first.key_sha256, &second.key_sha256] {
        assert!(contains(&lookup, digest).await.unwrap());
    }
    assert!(!contains(&lookup, &"d".repeat(64)).await.unwrap());
    writer.database(&database).drop().await.unwrap();
}
