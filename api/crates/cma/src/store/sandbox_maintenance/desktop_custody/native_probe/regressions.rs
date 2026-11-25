use super::super::{run_with_registry, Registry};
use super::*;
use layer2_core::sandbox::store::SandboxStoreError;
use tokio::sync::oneshot;

struct Held {
    caller: tokio::task::JoinHandle<std::result::Result<(), SandboxStoreError>>,
    entered: oneshot::Receiver<()>,
    commit: oneshot::Sender<()>,
    reconciling: oneshot::Receiver<()>,
    terminal: oneshot::Sender<()>,
    finished: oneshot::Receiver<()>,
}

fn start(id: SandboxId, registry: Arc<Registry>, refuse: bool) -> Held {
    let (entered_tx, entered) = oneshot::channel();
    let (commit, commit_rx) = oneshot::channel();
    let (reconciling_tx, reconciling) = oneshot::channel();
    let (terminal, terminal_rx) = oneshot::channel();
    let (finished_tx, finished) = oneshot::channel();
    let caller = tokio::spawn(async move {
        run_with_registry(
            registry,
            &id,
            Duration::from_secs(30),
            move |mut custody| async move {
                let commit_and_reconcile = async move {
                    custody.commit_started();
                    entered_tx.send(()).unwrap();
                    commit_rx.await.unwrap();
                    let watermark = mongodb::bson::Timestamp {
                        time: 1,
                        increment: 1,
                    };
                    custody
                        .reconcile(Some(watermark), move |at| async move {
                            assert_eq!(at, watermark);
                            reconciling_tx.send(()).unwrap();
                            terminal_rx.await.unwrap();
                            Ok(true)
                        })
                        .await
                        .unwrap();
                    drop(custody);
                    finished_tx.send(()).unwrap();
                    Ok::<(), SandboxStoreError>(())
                };
                if refuse {
                    // A refused response cannot reclaim custody already moved
                    // to a detached commit/reconciliation continuation.
                    drop(tokio::spawn(commit_and_reconcile));
                    Err(SandboxStoreError::Conflict)
                } else {
                    commit_and_reconcile.await
                }
            },
        )
        .await
    });
    Held {
        caller,
        entered,
        commit,
        reconciling,
        terminal,
        finished,
    }
}

async fn reached(signal: oneshot::Receiver<()>) {
    tokio::time::timeout(WAIT, signal)
        .await
        .expect("owned phase reached")
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn desktop_stays_excluded_through_detached_commit_and_reconciliation() {
    for exit in [
        "timeout_commit",
        "cancel_commit",
        "refusal",
        "timeout_reconciliation",
        "cancel_reconciliation",
    ] {
        let fixture = Fixture::new().await;
        let held = start(
            fixture.id.clone(),
            Arc::new(Registry::new(2)),
            exit == "refusal",
        );
        reached(held.entered).await;
        let mut commit = Some(held.commit);
        let mut reconciling = Some(held.reconciling);
        if exit.ends_with("reconciliation") {
            commit.take().unwrap().send(()).unwrap();
            reached(reconciling.take().unwrap()).await;
        }
        fixture.assert_blocked().await;
        match exit {
            "timeout_commit" | "timeout_reconciliation" => {
                tokio::time::advance(Duration::from_secs(31)).await;
                assert!(held.caller.await.unwrap().is_err());
            }
            "cancel_commit" | "cancel_reconciliation" => {
                held.caller.abort();
                assert!(held.caller.await.unwrap_err().is_cancelled());
            }
            "refusal" => assert!(matches!(
                held.caller.await.unwrap(),
                Err(SandboxStoreError::Conflict)
            )),
            _ => unreachable!(),
        }
        // The caller has ended while commit or its exact reconciliation is pending.
        fixture.assert_blocked().await;
        if let Some(commit) = commit {
            commit.send(()).unwrap();
            reached(reconciling.take().unwrap()).await;
        }
        fixture.assert_blocked().await;
        held.terminal.send(()).unwrap();
        reached(held.finished).await;
        fixture.assert_native_open().await;
    }
}

#[tokio::test(start_paused = true)]
async fn capacity_refusal_cannot_start_effects_or_release_existing_desktop_custody() {
    let first = Fixture::new().await;
    let second = Fixture::new().await;
    let registry = Arc::new(Registry::new(1));
    let held = start(first.id.clone(), registry.clone(), false);
    reached(held.entered).await;
    let calls = Arc::new(AtomicUsize::new(0));
    let attempted = calls.clone();
    let result = run_with_registry(registry, &second.id, WAIT, move |_| {
        attempted.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    })
    .await;
    assert!(result.is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    first.assert_blocked().await;
    second.assert_native_open().await;
    held.caller.abort();
    assert!(held.caller.await.unwrap_err().is_cancelled());
    held.commit.send(()).unwrap();
    reached(held.reconciling).await;
    first.assert_blocked().await;
    held.terminal.send(()).unwrap();
    reached(held.finished).await;
    first.assert_native_open().await;
}
