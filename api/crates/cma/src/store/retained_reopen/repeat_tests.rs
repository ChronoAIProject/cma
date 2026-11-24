use super::*;
use crate::store::sandbox_events::{event_id, read_event};

const RECEIPT: &str = "retained_reopen_receipt";

async fn seed_repeat(store: &MongoSandboxStore, name: &str) -> RetainedReopenRequest {
    let child = live(store, name, false).await;
    let snapshot = store
        .retained_suspended_reopen_snapshot(&child.sandbox_id)
        .await
        .unwrap();
    let request = RetainedSuspendedReopenRequest {
        repeat_authorized: false,
        proof: proof(&snapshot).unwrap(),
        snapshot,
        key_sha256: "a".repeat(64),
        requested_at: AT.into(),
    };
    store.admit_suspended_reopen(request).await.unwrap();
    save_failed_revival(store, &child.sandbox_id).await;
    let later = store
        .retained_reopen_snapshot(&child.sandbox_id)
        .await
        .unwrap();
    let mut request = legacy_request(later, "e".repeat(64));
    request.repeat_authorized = true;
    request
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_repeat_admission_serializes_identical_and_distinct_keys() {
    let (client, name, store) = database().await;
    let other_client =
        mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
    let other = MongoSandboxStore::open(&other_client, &name).await.unwrap();
    for (fixture, identical) in [("sbx_repeat_same", true), ("sbx_repeat_distinct", false)] {
        let first = seed_repeat(&store, fixture).await;
        let id = first.snapshot.retained.sandbox.id.clone();
        let mut competing = first.clone();
        if !identical {
            competing.key_sha256 = "f".repeat(64);
        }
        let before = store.sandbox_events(&id, 0, 10).await.unwrap();
        let old = row(&store, &id).await;
        let mut first_only = first.clone();
        first_only.repeat_authorized = false;
        assert!(matches!(
            store.admit_retained_reopen(first_only).await,
            Err(SandboxStoreError::Conflict)
        ));
        assert_eq!(row(&store, &id).await, old);
        assert_eq!(store.sandbox_events(&id, 0, 10).await.unwrap(), before);
        let (a, b) = tokio::join!(
            store.admit_retained_reopen(first.clone()),
            other.admit_retained_reopen(competing.clone())
        );
        let (winner, accepted) = match (a, b) {
            (Ok(a), Ok(b)) if identical => {
                assert_eq!(a, b);
                (first, a)
            }
            (Ok(a), Err(SandboxStoreError::Conflict)) if !identical => (first, a),
            (Err(SandboxStoreError::Conflict), Ok(b)) if !identical => (competing, b),
            pair => panic!("unexpected admission race: {pair:?}"),
        };
        let events = store.sandbox_events(&id, 0, 10).await.unwrap();
        assert_eq!(events.len(), before.len() + 1);
        let event = events.last().unwrap();
        let event_key = doc! { "_id": event_id(&id, event.seq) };
        let raw = store
            .sandbox_events
            .find_one(event_key.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read_event(&raw).unwrap(), *event);
        assert!(raw.contains_key(RECEIPT));
        let database = client.database(&name);
        for (index, mut filter, sort) in [
            (
                "sandbox_reopen_key_v1",
                doc! { "retained_reopen_receipt.key_sha256": &winner.key_sha256 },
                doc! {},
            ),
            (
                "sandbox_reopen_revision_v1",
                doc! {},
                doc! { "retained_reopen_receipt.accepted.revision": -1 },
            ),
        ] {
            filter.insert("sandbox_id", id.as_str());
            filter.insert(RECEIPT, doc! { "$exists": true });
            let explain = database
                .run_command(doc! {
                    "explain": { "find": "sandbox_events", "filter": filter,
                        "sort": sort, "hint": index, "limit": 1_i64 },
                    "verbosity": "executionStats",
                })
                .await
                .unwrap();
            let stats = explain.get_document("executionStats").unwrap();
            assert_eq!(stats.get_i32("totalKeysExamined").unwrap(), 1);
            assert!(stats.to_string().contains(index));
        }
        for field in [FIELD, SUSPENDED_FIELD] {
            assert_eq!(row(&store, &id).await.get(field), old.get(field));
        }
        // Retirement export includes the complete BSON of existing event envelopes.
        let bytes = mongodb::bson::serialize_to_vec(&raw).unwrap();
        let restored: Document = mongodb::bson::deserialize_from_slice(&bytes).unwrap();
        store
            .sandbox_events
            .replace_one(event_key, restored)
            .await
            .unwrap();
        let mut replay = winner;
        replay.repeat_authorized = false;
        assert_eq!(other.admit_retained_reopen(replay).await.unwrap(), accepted);
        assert_eq!(store.sandbox_events(&id, 0, 10).await.unwrap(), events);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_repeat_reconciliation_requires_the_exact_event_receipt() {
    let (client, name, store) = database().await;
    let request = seed_repeat(&store, "sbx_repeat_commit").await;
    let mut session = client.start_session().await.unwrap();
    let scope = TransactionScope::start(&mut session).await.unwrap();
    let transaction::Applied::Commit {
        receipt,
        event,
        repeat,
    } = transaction::apply_scoped(&store, &request, scope)
        .await
        .unwrap()
    else {
        panic!("repeat must write one event");
    };
    assert!(repeat);
    session.commit_transaction().await.unwrap();
    let time = session.operation_time().unwrap();
    transaction::reconcile(&store, &receipt, &event, time, true)
        .await
        .unwrap();
    let mut edited = (*receipt).clone();
    edited.key_sha256 = "c".repeat(64);
    assert!(transaction::reconcile(&store, &edited, &event, time, true)
        .await
        .is_err());
    assert!(
        transaction::reconcile(&store, &receipt, &event, time, false)
            .await
            .is_err()
    );
    store
        .sandbox_events
        .update_one(
            doc! { "_id": event_id(&event.sandbox_id, event.seq) },
            doc! { "$unset": { RECEIPT: 1 } },
        )
        .session(&mut session)
        .await
        .unwrap();
    let without_receipt = session.operation_time().unwrap();
    assert!(
        transaction::reconcile(&store, &receipt, &event, without_receipt, true)
            .await
            .is_err()
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn retained_repeat_receipt_replays_from_retirement_archive() {
    use crate::retirement_archive::{export, tests::Custody, verify};
    use crate::store::retirement_archive::prepare_source;

    let (client, name, store) = database().await;
    let request = seed_repeat(&store, "sbx_repeat_archive").await;
    let id = &request.snapshot.retained.sandbox.id;
    let accepted = store.admit_retained_reopen(request.clone()).await.unwrap();
    let (temp, archive, retirement, bytes) =
        prepare_source(client.clone(), client.database(&name), id.as_str()).await;
    archive.admit(&retirement, &bytes).await.unwrap();
    let custody = Custody::default();
    let manifest = export::publish(&archive, &retirement, &bytes, &custody, None)
        .await
        .unwrap();
    let directory = temp.0.join("restore");
    verify::restore(&custody, &manifest, &retirement, &bytes, &directory)
        .await
        .unwrap();

    let restored_name = format!("{}_restore", name);
    let restored = MongoSandboxStore::open(&client, &restored_name)
        .await
        .unwrap();
    for collection in [&restored.sandboxes, &restored.sandbox_events] {
        let file = directory
            .join("database")
            .join(format!("{}.bson", collection.name()));
        let bytes = std::fs::read(file).unwrap();
        let mut rows = bytes.as_slice();
        while !rows.is_empty() {
            collection
                .insert_one(Document::from_reader(&mut rows).unwrap())
                .await
                .unwrap();
        }
    }
    assert_eq!(
        restored.admit_retained_reopen(request).await.unwrap(),
        accepted
    );
    client.database(&restored_name).drop().await.unwrap();
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn suspended_repeat_requires_transaction_permission_but_exact_replay_does_not() {
    let (client, name, store) = database().await;
    let child = live(&store, "sbx_suspended_repeat", false).await;
    let id = &child.sandbox_id;
    let snapshot = store.retained_suspended_reopen_snapshot(id).await.unwrap();
    let original = RetainedSuspendedReopenRequest {
        repeat_authorized: false,
        proof: proof(&snapshot).unwrap(),
        snapshot,
        key_sha256: "a".repeat(64),
        requested_at: AT.into(),
    };
    let first = store
        .admit_suspended_reopen(original.clone())
        .await
        .unwrap();
    save_failed_revival(&store, id).await;

    let parent = store.get_sandbox(id).await.unwrap().unwrap();
    let generation = parent.runtime.unwrap().provision_generation.unwrap();
    let child = store.get_agent(&child.id).await.unwrap().unwrap();
    let child = set_live_child(&store, child, generation, false).await;
    let snapshot = store.retained_suspended_reopen_snapshot(id).await.unwrap();
    assert!(snapshot.retained.sandbox.revision > first.revision);
    let mut request = RetainedSuspendedReopenRequest {
        repeat_authorized: false,
        proof: proof(&snapshot).unwrap(),
        snapshot,
        key_sha256: "b".repeat(64),
        requested_at: AT.into(),
    };
    let before = row(&store, id).await;
    let events = store.sandbox_events(id, 0, 10).await.unwrap();
    assert!(matches!(
        store.admit_suspended_reopen(request.clone()).await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(row(&store, id).await, before);
    assert_eq!(store.get_agent(&child.id).await.unwrap().unwrap(), child);
    assert_eq!(store.sandbox_events(id, 0, 10).await.unwrap(), events);
    assert_eq!(store.admit_suspended_reopen(original).await.unwrap(), first);

    request.repeat_authorized = true;
    let accepted = store.admit_suspended_reopen(request.clone()).await.unwrap();
    request.repeat_authorized = false;
    assert_eq!(
        store.admit_suspended_reopen(request).await.unwrap(),
        accepted
    );
    assert_eq!(
        store.sandbox_events(id, 0, 10).await.unwrap().len(),
        events.len() + 1
    );
    client.database(&name).drop().await.unwrap();
}
