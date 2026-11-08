use super::*;
use crate::ledger_storage::fault_vfs::{Event, Vfs};
use crate::mutation_ledger::tests::{close, Fixture};

#[tokio::test]
async fn import_commit_faults_preserve_atomic_raw_results_and_never_clear_the_hold() {
    for event in [
        Event::JournalWrite,
        Event::JournalSync,
        Event::DatabaseWrite,
        Event::DatabaseSync,
        Event::Delete(true),
    ] {
        for status in [
            Status::Started,
            Status::Completed,
            Status::Failed,
            Status::Indeterminate,
        ] {
            let fixture = Fixture::new();
            let vfs = Vfs::new();
            let storage = Storage::open_fault_vfs(fixture.database(), true, 32_768, vfs.name())
                .await
                .unwrap();
            let config = fixture.config.clone();
            storage
                .call(move |db| schema::initialize_held(db, &config))
                .await
                .unwrap();
            let store = HeldImport {
                storage,
                config: fixture.config.clone(),
                directory: fixture.database(),
                anchor: None,
            };
            let started = Record::started(&fixture.config, &close()).unwrap();
            let record = match status {
                Status::Started => started,
                Status::Completed => started
                    .completed(&layer3_adapter::wire::AgentCloseReceipt {
                        operation_id: close().operation_id,
                        request_sha256: close().request_sha256,
                        agent_id: layer3_adapter::AgentSlot::new("agent"),
                    })
                    .unwrap(),
                _ => started
                    .failed(
                        &crate::failure::RuntimeError::rejected(
                            if status == Status::Indeterminate {
                                "agent_close_outcome_unknown"
                            } else {
                                "effect_failed"
                            },
                            "effect outcome",
                        ),
                        status == Status::Indeterminate,
                    )
                    .unwrap(),
            };
            let bytes = serde_json::to_vec_pretty(&record).unwrap();
            vfs.state.arm(event);
            assert_eq!(
                store.journal(0, bytes.clone()).await.unwrap_err().code,
                "mutation_ledger_storage_hold"
            );
            assert!(vfs.state.events().contains(&event));
            store.close().await.unwrap();
            assert_eq!(
                crate::ledger_storage::reserve::remaining(&fixture.database()).unwrap(),
                0
            );
            let storage = Storage::open_fault_vfs(fixture.database(), false, 32_768, vfs.name())
                .await
                .unwrap();
            storage
                .call(move |db| {
                    let admission: String = db
                        .query_row("SELECT admission FROM metadata", [], |r| r.get(0))
                        .unwrap();
                    assert_eq!(admission, "import_held");
                    let count: i64 = db
                        .query_row("SELECT count(*) FROM operations", [], |r| r.get(0))
                        .unwrap();
                    assert!(count == 0 || count == 1);
                    if count == 1 {
                        let raw: Vec<u8> = db
                            .query_row("SELECT record FROM operations", [], |r| r.get(0))
                            .unwrap();
                        assert_eq!(raw, bytes);
                    }
                    verify_counters(db, 0)?;
                    Ok(())
                })
                .await
                .unwrap();
            let reopened = HeldImport {
                storage,
                config: fixture.config.clone(),
                directory: fixture.database(),
                anchor: None,
            };
            assert_eq!(
                reopened.finish(0).await.unwrap_err().code,
                "mutation_ledger_storage_hold"
            );
        }
    }
}

#[tokio::test]
async fn held_import_rejects_foreign_scopes_and_duplicate_journal_results() {
    let fixture = Fixture::new();
    let store = HeldImport::create(fixture.database(), fixture.config.clone(), 32_768)
        .await
        .unwrap();
    let bytes = Record::started(&fixture.config, &close())
        .unwrap()
        .encode()
        .unwrap();
    assert!(store
        .journal(fixture.config.generation + 1, bytes.clone())
        .await
        .is_err());
    assert!(store.journal(1, bytes.clone()).await.is_err()); // Explicit legacy config has persistent scope.
    store.journal(0, bytes.clone()).await.unwrap();
    assert!(store.journal(0, bytes).await.is_err());
    let summary = store.finish(0).await.unwrap();
    assert_eq!(summary["operations"], 1);
    assert_eq!(summary["admission"], "import_held");
}

#[tokio::test]
async fn held_import_cannot_qualify_a_different_captured_generation() {
    let fixture = Fixture::new();
    let mut config = fixture.config.clone();
    config.generation = 7;
    let store = HeldImport::create(fixture.database(), config, 32_768)
        .await
        .unwrap();
    store
        .storage
        .call(|db| {
            db.execute(
                "UPDATE metadata SET generation=?",
                [6u64.to_be_bytes().as_slice()],
            )
            .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        store.finish(0).await.unwrap_err().code,
        "mutation_ledger_invalid"
    );
    let db = rusqlite::Connection::open_with_flags(
        fixture.database().join("ledger.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let (admission, generation): (String, Vec<u8>) = db
        .query_row("SELECT admission,generation FROM metadata", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(admission, "import_held");
    assert_eq!(generation, 6u64.to_be_bytes());
}
