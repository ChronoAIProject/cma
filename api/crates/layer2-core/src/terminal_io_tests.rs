use std::sync::Arc;
use std::time::Duration;

use layer3_adapter::{AdapterError, TerminalInput, TerminalInputSender};

use crate::commit_fault_support::CommitFaultStore;
use crate::coordination::SessionOperation;
use crate::error::CoreError;
use crate::event::{Event, EventKind};
use crate::store::{SessionRecord, SessionStore};
use crate::terminal::TerminalGeometry;
use crate::test_support::{
    await_settled, service, service_over, spec, terminal_input_channel, user, FakeAdapter,
    FakeStore,
};

fn is_resize_commit(_record: &SessionRecord, events: &[Event]) -> bool {
    events
        .iter()
        .any(|event| matches!(event.kind, EventKind::TerminalResized { .. }))
}

async fn wait_for_interaction_lease(
    store: &FakeStore,
    id: &crate::ids::SessionId,
) -> crate::coordination::SessionLease {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(lease) = store
                .active_session_lease(id)
                .await
                .filter(|lease| lease.operation == SessionOperation::Interact)
            {
                return lease;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the resize acquires its interaction lease")
}

async fn wait_for_interaction_release(store: &FakeStore, id: &crate::ids::SessionId) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while store.active_session_lease(id).await.is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the resize releases its interaction lease");
}

#[tokio::test]
async fn a_second_service_readopts_before_fenced_resize_delivery() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let first = service(Arc::clone(&store), Arc::clone(&adapter));
    let owner = user("resize-readoption-owner");
    let created = first.create_session(&owner, spec()).await.expect("created");
    await_settled(&first, &owner, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();

    let second = service(Arc::clone(&store), Arc::clone(&adapter));
    assert!(!second.entries.read().await.contains_key(&created.id));
    let geometry = TerminalGeometry::new(101, 31).expect("valid geometry");
    second
        .resize_terminal(&owner, &created.id, geometry)
        .await
        .expect("the second service readopts and resizes the exact runtime");
    wait_for_interaction_release(&store, &created.id).await;

    let delivered = adapter.terminal_inputs.lock().await;
    assert!(matches!(
        delivered.as_slice(),
        [(_, TerminalInput::Resize(size))]
            if size.cols == geometry.columns() && size.rows == geometry.rows()
    ));
    drop(delivered);
    assert_eq!(
        second
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("the readopted entry is live")
            .session
            .geometry,
        geometry
    );
    assert_eq!(
        second
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("the readopted screen is visible")
            .geometry,
        geometry
    );
}

#[tokio::test]
async fn a_stale_resize_generation_reaches_neither_store_screen_nor_sender() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&store), Arc::clone(&adapter));
    let owner = user("stale-resize-generation-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();
    let before = store.get(&created.id).await.unwrap().unwrap();
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is visible");
    let entry_generation = {
        let mut entries = service.entries.write().await;
        let entry = entries.get_mut(&created.id).expect("the session is live");
        entry.runtime_generation = entry
            .runtime_generation
            .checked_add(1)
            .expect("test generation remains bounded");
        entry.runtime_generation
    };

    assert_eq!(
        service
            .resize_terminal(
                &owner,
                &created.id,
                TerminalGeometry::new(102, 32).expect("valid geometry"),
            )
            .await,
        Err(CoreError::RuntimeUnavailable(
            "the live terminal belongs to another runtime generation".into(),
        ))
    );
    wait_for_interaction_release(&store, &created.id).await;

    let durable = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, before.session.geometry);
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    assert!(store
        .events_since(&created.id, before.session.last_seq)
        .await
        .unwrap()
        .is_empty());
    let entries = service.entries.read().await;
    let entry = entries
        .get(&created.id)
        .expect("the stale entry remains live");
    assert_eq!(entry.runtime_generation, entry_generation);
    assert_eq!(entry.session.geometry, before.session.geometry);
    drop(entries);
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains visible")
            .geometry,
        before_screen.geometry
    );
    assert!(adapter.terminal_inputs.lock().await.is_empty());
}

#[tokio::test]
async fn resize_sender_rejection_returns_error_and_retry_reconciles_desired_geometry() {
    let store = Arc::new(FakeStore::default());
    let service = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    let owner = user("rejected-resize-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    let before = store.get(&created.id).await.unwrap().unwrap();
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is visible");
    let rejected = TerminalInputSender::spawn(1, |_input| async {
        Err(AdapterError::Rejected(
            "the runtime refused its resize".into(),
        ))
    });
    service
        .entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("the session is live")
        .input = Some(rejected);
    let desired = TerminalGeometry::new(103, 33).expect("valid geometry");

    assert_eq!(
        service.resize_terminal(&owner, &created.id, desired).await,
        Err(CoreError::RuntimeRefused(
            "the runtime refused its resize".into(),
        ))
    );
    wait_for_interaction_release(&store, &created.id).await;

    let durable = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, desired);
    assert_eq!(durable.session.last_seq, before.session.last_seq + 1);
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("the live entry remains")
            .session
            .geometry,
        before.session.geometry,
        "a rejected runtime receipt does not publish live geometry"
    );
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains visible")
            .geometry,
        before_screen.geometry,
        "a rejected runtime receipt does not resize the local screen"
    );

    let (accepted, mut runtime_input) = terminal_input_channel(1);
    service
        .entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("the live entry remains")
        .input = Some(accepted);
    service
        .resize_terminal(&owner, &created.id, desired)
        .await
        .expect("retry applies the durable desired geometry to the live runtime");
    wait_for_interaction_release(&store, &created.id).await;
    assert!(matches!(
        runtime_input.recv().await,
        Some(TerminalInput::Resize(size))
            if size.cols == desired.columns() && size.rows == desired.rows()
    ));
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains visible")
            .geometry,
        desired
    );
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .last_seq,
        before.session.last_seq + 2
    );
}

#[tokio::test]
async fn a_stalled_resize_sender_does_not_hold_the_service_entry_map() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&store), Arc::clone(&adapter));
    let owner = user("independent-resize-owner");
    let resizing = service
        .create_session(&owner, spec())
        .await
        .expect("resizing session created");
    let independent = service
        .create_session(&owner, spec())
        .await
        .expect("independent session created");
    await_settled(&service, &owner, &resizing.id).await;
    await_settled(&service, &owner, &independent.id).await;

    adapter.pause_next_terminal_input();
    let resize = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = resizing.id.clone();
        async move {
            service
                .resize_terminal(
                    &owner,
                    &id,
                    TerminalGeometry::new(111, 47).expect("valid geometry"),
                )
                .await
        }
    });
    adapter.wait_until_terminal_input_paused().await;

    assert_eq!(
        tokio::time::timeout(
            Duration::from_millis(250),
            service.terminal_is_held(&owner, &independent.id),
        )
        .await
        .expect("another session can still read the service entry map")
        .expect("the independent session remains readable"),
        false
    );

    adapter.resume_terminal_input();
    resize
        .await
        .expect("resize task remains live")
        .expect("resize completes after its sender resumes");
}

#[tokio::test]
async fn a_sender_replaced_after_resize_commit_receives_nothing_until_retry() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_resize_commit));
    let service = service_over(store.clone(), Arc::new(FakeAdapter::default()));
    let owner = user("replaced-resize-sender-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    let before = inner.get(&created.id).await.unwrap().unwrap();
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is visible");
    let desired = TerminalGeometry::new(104, 34).expect("valid geometry");
    let (outgoing, mut outgoing_runtime) = terminal_input_channel(1);
    service
        .entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("the session is live")
        .input = Some(outgoing);

    store.pause_after_next();
    let resizing = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.resize_terminal(&owner, &id, desired).await }
    });
    store.wait_until_held().await;
    let (successor, mut successor_runtime) = terminal_input_channel(1);
    service
        .entries
        .write()
        .await
        .get_mut(&created.id)
        .expect("the session is live")
        .input = Some(successor);
    store.release();

    assert!(matches!(
        resizing.await.expect("resize task completes"),
        Err(CoreError::RuntimeUnavailable(_))
    ));
    assert_eq!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .geometry,
        desired,
        "the committed geometry remains the desired successor state"
    );
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("the live entry remains")
            .session
            .geometry,
        before.session.geometry
    );
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains visible")
            .geometry,
        before_screen.geometry
    );
    assert!(matches!(
        outgoing_runtime.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty
            | tokio::sync::mpsc::error::TryRecvError::Disconnected)
    ));
    assert!(matches!(
        successor_runtime.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));

    service
        .resize_terminal(&owner, &created.id, desired)
        .await
        .expect("retry reconciles the desired geometry on the successor sender");
    assert!(matches!(
        successor_runtime.recv().await,
        Some(TerminalInput::Resize(size))
            if size.cols == desired.columns() && size.rows == desired.rows()
    ));
}

#[tokio::test]
async fn cancellation_before_resize_admission_reaches_neither_store_screen_nor_sender() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&store), Arc::clone(&adapter));
    let owner = user("cancelled-before-resize-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();
    let before = store.get(&created.id).await.unwrap().unwrap();
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is visible");
    let input_fence = service.input_gates.gate(&created.id).write_owned().await;

    let resizing = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move {
            service
                .resize_terminal(
                    &owner,
                    &id,
                    TerminalGeometry::new(105, 35).expect("valid geometry"),
                )
                .await
        }
    });
    let held = wait_for_interaction_lease(&store, &created.id).await;
    assert_eq!(held.operation, SessionOperation::Interact);
    resizing.abort();
    assert!(resizing.await.unwrap_err().is_cancelled());
    drop(input_fence);
    wait_for_interaction_release(&store, &created.id).await;

    let durable = store.get(&created.id).await.unwrap().unwrap();
    assert_eq!(durable.session.geometry, before.session.geometry);
    assert_eq!(durable.session.last_seq, before.session.last_seq);
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains visible")
            .geometry,
        before_screen.geometry
    );
    assert!(adapter.terminal_inputs.lock().await.is_empty());
}

#[tokio::test]
async fn cancellation_after_resize_commit_starts_no_runtime_delivery() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), is_resize_commit));
    let adapter = Arc::new(FakeAdapter::default());
    let service = service_over(store.clone(), Arc::clone(&adapter));
    let owner = user("cancelled-committed-resize-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();
    let before = inner.get(&created.id).await.unwrap().unwrap();
    let before_screen = service
        .screen_snapshot(&owner, &created.id)
        .await
        .expect("screen is visible");
    let desired = TerminalGeometry::new(107, 37).expect("valid geometry");

    store.pause_after_next();
    let resizing = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.resize_terminal(&owner, &id, desired).await }
    });
    store.wait_until_held().await;
    assert_eq!(
        inner
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .geometry,
        desired,
        "the resize is durable before its final delivery check"
    );
    resizing.abort();
    assert!(resizing.await.unwrap_err().is_cancelled());
    store.release();
    drop(
        tokio::time::timeout(Duration::from_secs(2), service.lock_lifecycle(&created.id))
            .await
            .expect("a public lock restarts and drains the retained continuation"),
    );

    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("the live entry remains")
            .session
            .geometry,
        before.session.geometry
    );
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains visible")
            .geometry,
        before_screen.geometry
    );
    assert!(adapter.terminal_inputs.lock().await.is_empty());
}

#[tokio::test]
async fn cancellation_after_resize_admission_retains_receipt_and_live_finalization() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let service = service(Arc::clone(&store), Arc::clone(&adapter));
    let owner = user("cancelled-after-resize-owner");
    let created = service
        .create_session(&owner, spec())
        .await
        .expect("created");
    await_settled(&service, &owner, &created.id).await;
    adapter.terminal_inputs.lock().await.clear();
    let before = store.get(&created.id).await.unwrap().unwrap();
    let desired = TerminalGeometry::new(106, 36).expect("valid geometry");
    adapter.pause_next_terminal_input();

    let resizing = tokio::spawn({
        let service = Arc::clone(&service);
        let owner = owner.clone();
        let id = created.id.clone();
        async move { service.resize_terminal(&owner, &id, desired).await }
    });
    adapter.wait_until_terminal_input_paused().await;
    resizing.abort();
    assert!(resizing.await.unwrap_err().is_cancelled());
    adapter.resume_terminal_input();
    wait_for_interaction_release(&store, &created.id).await;

    assert!(matches!(
        adapter.terminal_inputs.lock().await.as_slice(),
        [(_, TerminalInput::Resize(size))]
            if size.cols == desired.columns() && size.rows == desired.rows()
    ));
    assert_eq!(
        store
            .get(&created.id)
            .await
            .unwrap()
            .unwrap()
            .session
            .last_seq,
        before.session.last_seq + 1
    );
    assert_eq!(
        service
            .entries
            .read()
            .await
            .get(&created.id)
            .expect("the live entry remains")
            .session
            .geometry,
        desired
    );
    assert_eq!(
        service
            .screen_snapshot(&owner, &created.id)
            .await
            .expect("screen remains visible")
            .geometry,
        desired
    );
}
