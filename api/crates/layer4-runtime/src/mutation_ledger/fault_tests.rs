use super::tests::{close, Fixture};
use super::*;
use crate::ledger_storage::fault_vfs::{Event, Vfs};
use crate::mutation_journal::Status;
use layer3_adapter::{wire::AgentCloseReceipt, AgentSlot};

async fn open(fixture: &Fixture, vfs: &Vfs, create: bool) -> Ledger {
    let storage = Storage::open_fault_vfs(fixture.database(), create, 32_768, vfs.name())
        .await
        .unwrap();
    let config = fixture.config.clone();
    storage
        .call(move |db| schema::open(db, &config, create))
        .await
        .unwrap();
    Ledger {
        storage,
        config: Arc::new(fixture.config.clone()),
        directory: fixture.database(),
        guard: None,
    }
}
fn receipt() -> AgentCloseReceipt {
    AgentCloseReceipt {
        operation_id: close().operation_id,
        request_sha256: close().request_sha256,
        agent_id: AgentSlot::new("agent"),
    }
}

#[tokio::test]
async fn extra_commit_orders_journal_sync_database_sync_then_synced_directory_delete() {
    let fixture = Fixture::new();
    let vfs = Vfs::new();
    let ledger = open(&fixture, &vfs, true).await;
    vfs.state.clear();
    ledger.admit(1, vec![close()]).await.unwrap();
    let events = vfs.state.events();
    let journal = events
        .iter()
        .position(|e| *e == Event::JournalSync)
        .unwrap();
    let write = events
        .iter()
        .position(|e| *e == Event::DatabaseWrite)
        .unwrap();
    let database = events
        .iter()
        .position(|e| *e == Event::DatabaseSync)
        .unwrap();
    let delete = events
        .iter()
        .position(|e| *e == Event::Delete(true))
        .unwrap();
    assert!(
        journal < write && write < database && database < delete,
        "{events:?}"
    );
    assert!(
        !events.contains(&Event::Delete(false)),
        "EXTRA must sync the unlink: {events:?}"
    );
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn full_and_sync_failures_at_each_transaction_boundary_hold_and_reopen_atomically() {
    for event in [
        Event::JournalWrite,
        Event::JournalSync,
        Event::DatabaseWrite,
        Event::DatabaseSync,
        Event::Delete(true),
    ] {
        for boundary in ["admission", "started", "terminal"] {
            let fixture = Fixture::new();
            let vfs = Vfs::new();
            let ledger = open(&fixture, &vfs, true).await;
            if boundary != "admission" {
                ledger.admit(1, vec![close()]).await.unwrap();
            }
            let terminal = if boundary == "terminal" {
                Some(
                    ledger
                        .begin(close())
                        .await
                        .unwrap()
                        .record
                        .completed(&receipt())
                        .unwrap(),
                )
            } else {
                None
            };
            vfs.state.arm(event);
            let result = match boundary {
                "admission" => ledger.admit(1, vec![close()]).await,
                "started" => ledger.begin(close()).await.map(|_| ()),
                _ => ledger.finish(terminal.clone().unwrap()).await,
            };
            assert_eq!(
                result.unwrap_err().code,
                "mutation_ledger_storage_hold",
                "{boundary} {event:?}"
            );
            assert!(vfs.state.events().contains(&event));
            assert_eq!(
                crate::ledger_storage::reserve::remaining(&fixture.database()).unwrap(),
                0
            );
            ledger.close().await.unwrap();
            let ledger = open(&fixture, &vfs, false).await;
            let snapshot = ledger.snapshot().await.unwrap();
            assert_eq!(snapshot.emergency_remaining_bytes, 0);
            if boundary == "admission" {
                let authority = ledger.recovery_fence(close()).await.unwrap();
                assert_eq!(snapshot.operations, i64::from(authority.is_some()));
                assert_eq!(snapshot.authority, snapshot.operations);
                assert!(!ledger.admitted(close()).await.unwrap());
                assert_eq!(snapshot.terminal, 0);
            } else if boundary == "started" {
                assert_eq!((snapshot.operations, snapshot.terminal), (1, 0));
                let record = ledger.begin(close()).await.unwrap().record;
                assert_eq!(record.status(), Status::Started);
            } else {
                // The effect would already have happened once. Inspect before
                // retrying its terminal commit; do not rerun the external effect.
                let inspected = ledger.begin(close()).await.unwrap().record;
                assert!(matches!(
                    inspected.status(),
                    Status::Started | Status::Completed
                ));
                if inspected.status() == Status::Started {
                    ledger.finish(terminal.unwrap()).await.unwrap();
                }
                let replay = ledger.begin(close()).await.unwrap().record;
                assert_eq!(replay.receipt::<AgentCloseReceipt>().unwrap(), receipt());
                assert_eq!(ledger.snapshot().await.unwrap().terminal, 1);
            }
            ledger.close().await.unwrap();
        }
    }
}

#[tokio::test]
async fn directory_sync_failure_after_unlink_never_becomes_an_acknowledged_terminal() {
    let fixture = Fixture::new();
    let vfs = Vfs::new();
    let ledger = open(&fixture, &vfs, true).await;
    ledger.admit(1, vec![close()]).await.unwrap();
    let terminal = ledger
        .begin(close())
        .await
        .unwrap()
        .record
        .completed(&receipt())
        .unwrap();
    vfs.state.arm(Event::Delete(true));
    assert!(ledger.finish(terminal).await.is_err());
    assert!(vfs.state.events().contains(&Event::DatabaseSync));
    ledger.close().await.unwrap();
    let ledger = open(&fixture, &vfs, false).await;
    // The simulated deletion happened. Reopen barriers make the visible
    // committed row durable, and it must replay exactly without another effect.
    ledger.admit(2, vec![close()]).await.unwrap();
    assert_eq!(
        ledger
            .begin(close())
            .await
            .unwrap()
            .record
            .receipt::<AgentCloseReceipt>()
            .unwrap(),
        receipt()
    );
    assert_eq!(
        ledger.snapshot().await.unwrap().emergency_remaining_bytes,
        0
    );
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn extra_survives_namespace_loss_while_full_control_loses_acknowledged_receipt() {
    for extra in [false, true] {
        let fixture = Fixture::new();
        let vfs = Vfs::new();
        let ledger = open(&fixture, &vfs, true).await;
        ledger.admit(1, vec![close()]).await.unwrap();
        let terminal = ledger
            .begin(close())
            .await
            .unwrap()
            .record
            .completed(&receipt())
            .unwrap();
        if !extra {
            // Deliberate negative control, never used by a serving connection.
            ledger
                .storage
                .call(|db| {
                    db.pragma_update(None, "synchronous", "FULL").unwrap();
                    Ok(())
                })
                .await
                .unwrap();
        }
        vfs.state.model_namespace_loss();
        ledger.finish(terminal).await.unwrap(); // Acknowledged transaction.
        ledger.close().await.unwrap();
        assert_eq!(vfs.state.lose_unsynced_namespace(), !extra);
        let ledger = open(&fixture, &vfs, false).await;
        let record = ledger.begin(close()).await.unwrap().record;
        let snapshot = ledger.snapshot().await.unwrap();
        if extra {
            assert_eq!(record.receipt::<AgentCloseReceipt>().unwrap(), receipt());
            assert_eq!((snapshot.terminal, snapshot.pending), (1, 0));
        } else {
            assert_eq!(record.status(), Status::Started);
            assert_eq!((snapshot.terminal, snapshot.pending), (0, 1));
        }
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn failed_extra_barrier_then_namespace_loss_recovers_started_without_repeating_effect() {
    let fixture = Fixture::new();
    let vfs = Vfs::new();
    let ledger = open(&fixture, &vfs, true).await;
    ledger.admit(1, vec![close()]).await.unwrap();
    let terminal = ledger
        .begin(close())
        .await
        .unwrap()
        .record
        .completed(&receipt())
        .unwrap();
    vfs.state.model_namespace_loss();
    vfs.state.arm(Event::Delete(true));
    assert_eq!(
        ledger.finish(terminal.clone()).await.unwrap_err().code,
        "mutation_ledger_storage_hold"
    );
    ledger.close().await.unwrap();
    assert!(vfs.state.lose_unsynced_namespace());
    let ledger = open(&fixture, &vfs, false).await;
    let recovered = ledger.begin(close()).await.unwrap();
    assert!(!recovered.new);
    assert_eq!(recovered.record.status(), Status::Started);
    assert_eq!(ledger.snapshot().await.unwrap().terminal, 0);
    // The parent reconciles the already-completed effect. Recommit only its
    // receipt; a lost terminal transaction never authorizes another effect.
    ledger.finish(terminal).await.unwrap();
    assert_eq!(
        ledger
            .begin(close())
            .await
            .unwrap()
            .record
            .receipt::<AgentCloseReceipt>()
            .unwrap(),
        receipt()
    );
    assert_eq!(
        ledger.snapshot().await.unwrap().emergency_remaining_bytes,
        0
    );
    ledger.close().await.unwrap();
}
