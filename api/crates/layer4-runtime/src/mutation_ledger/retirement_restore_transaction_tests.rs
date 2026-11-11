use super::super::transaction::{restore_rows, RestoredSource};
use super::*;

async fn restore(
    fixture: &Fixture,
    source: &Ledger,
    retained: &Retained,
    transition: &DeactivationReceipt,
    commit: bool,
) -> Result<RestoredSource> {
    let config = fixture.config.clone();
    let selection = retained.selection(fixture);
    let transition = transition.clone();
    source
        .test_retirement_storage(move |db| {
            let tx = db
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(|_| crate::mutation_ledger::storage_error())?;
            let outcome = restore_rows(&tx, &config, &selection, &transition)?;
            if commit { tx.commit() } else { tx.rollback() }
                .map_err(|_| crate::mutation_ledger::storage_error())?;
            Ok(outcome)
        })
        .await
}

async fn rows(source: &Ledger, config: &crate::config::RuntimeConfig) -> Vec<Vec<u8>> {
    let mut cursor = None;
    let mut result = Vec::new();
    loop {
        let page = source.retirement_inventory(cursor).await.unwrap();
        result.extend(page.rows.iter().map(|row| row.encode(config).unwrap()));
        cursor = page.next;
        if cursor.is_none() {
            result.sort();
            return result;
        }
    }
}

#[tokio::test]
async fn transactional_restore_preserves_original_columns_and_advances_revision_after_reopen() {
    let (fixture, source, retained, transition) = prepare_source(true).await;
    let before = source.snapshot().await.unwrap();
    let mut expected = retained
        .rows
        .iter()
        .map(|row| row.encode(&fixture.config).unwrap())
        .collect::<Vec<_>>();
    expected.extend(rows(&source, &fixture.config).await);
    expected.sort();
    let restored = restore(&fixture, &source, &retained, &transition, true)
        .await
        .unwrap();
    assert_eq!(restored.before_sha256, transition.after_sha256);
    assert_eq!(restored.source_sha256, transition.source_sha256);
    assert_eq!(restored.before_revision, transition.after_revision);
    assert_eq!(restored.after_revision, transition.after_revision + 1);
    assert_eq!(
        (restored.operations, restored.authority, restored.cleanup),
        (4, 2, 2)
    );
    assert_eq!(rows(&source, &fixture.config).await, expected);
    source.close().await.unwrap();
    let source = fixture.open(false).await;
    let after = source.snapshot().await.unwrap();
    assert_eq!(rows(&source, &fixture.config).await, expected);
    assert_eq!(after.revision, before.revision + 1);
    assert_eq!(after.operations, before.operations + 4);
    assert_eq!(after.authority, before.authority + 2);
    assert_eq!(after.terminal, before.terminal + 4);
    assert_eq!(
        (after.reserved_bytes, after.pending, after.processes),
        (before.reserved_bytes, before.pending, before.processes)
    );
    // No second mutation may use the original after-state. The future held
    // coordinator must resolve an exact retry from its durable outcome slot.
    let before_retry = bytes(&fixture);
    assert_eq!(
        restore(&fixture, &source, &retained, &transition, true)
            .await
            .unwrap_err()
            .code,
        "mutation_ledger_restore_source_changed"
    );
    assert_eq!(bytes(&fixture), before_retry);
    source.close().await.unwrap();
}

#[tokio::test]
async fn rolling_back_after_all_rows_and_counters_restores_the_exact_prior_database() {
    let (fixture, source, retained, transition) = prepare().await;
    let before = bytes(&fixture);
    let snapshot = source.snapshot().await.unwrap();
    let result = restore(&fixture, &source, &retained, &transition, false)
        .await
        .unwrap();
    assert_eq!(result.after_revision, transition.after_revision + 1);
    assert_eq!(source.snapshot().await.unwrap(), snapshot);
    assert_eq!(bytes(&fixture), before);
    source.close().await.unwrap();
    let source = fixture.open(false).await;
    assert_eq!(source.snapshot().await.unwrap(), snapshot);
    restore(&fixture, &source, &retained, &transition, true)
        .await
        .unwrap();
    source.close().await.unwrap();
}

#[tokio::test]
async fn insert_and_accounting_failures_rollback_all_rows_and_counters() {
    for sql in [
        "CREATE TEMP TRIGGER fail_restore BEFORE INSERT ON main.authority BEGIN SELECT RAISE(ABORT,'injected'); END",
        "CREATE TEMP TRIGGER fail_restore BEFORE UPDATE ON main.accounting BEGIN SELECT RAISE(ABORT,'injected'); END",
    ] {
        let (fixture, source, retained, transition) = prepare().await;
        source.test_retirement_storage(move |db| {
            db.execute_batch(sql).unwrap();
            Ok(())
        }).await.unwrap();
        let before = bytes(&fixture);
        let snapshot = source.snapshot().await.unwrap();
        let originals = rows(&source, &fixture.config).await;
        assert_eq!(restore(&fixture, &source, &retained, &transition, true).await.unwrap_err().code,
            "mutation_ledger_storage_hold");
        let after = source.snapshot().await.unwrap();
        assert_eq!((after.revision, after.operations, after.authority, after.terminal, after.reserved_bytes),
            (snapshot.revision, snapshot.operations, snapshot.authority, snapshot.terminal, snapshot.reserved_bytes));
        assert_eq!(rows(&source, &fixture.config).await, originals);
        assert_eq!(bytes(&fixture).0, before.0);
        // Genuine write failures keep the existing durable storage hold behavior.
        assert_eq!(after.emergency_remaining_bytes, 0);
        source.test_retirement_storage(|db| {
            db.execute_batch("DROP TRIGGER temp.fail_restore").unwrap();
            Ok(())
        }).await.unwrap();
        source.close().await.unwrap();
        let source = fixture.open(false).await;
        assert_eq!(rows(&source, &fixture.config).await, originals);
        source.close().await.unwrap();
    }
}

#[tokio::test]
async fn later_effects_and_changed_receipts_refuse_before_any_restore_write() {
    for later_work in [false, true] {
        let (fixture, source, retained, mut transition) = prepare().await;
        if later_work {
            cycle(&fixture, &source, "later").await;
        } else {
            transition.source_sha256 = "0".repeat(64);
        }
        let before = bytes(&fixture);
        let snapshot = source.snapshot().await.unwrap();
        assert_eq!(
            restore(&fixture, &source, &retained, &transition, true)
                .await
                .unwrap_err()
                .code,
            "mutation_ledger_restore_source_changed"
        );
        assert_eq!(source.snapshot().await.unwrap(), snapshot);
        assert_eq!(bytes(&fixture), before);
        source.close().await.unwrap();
    }
}
