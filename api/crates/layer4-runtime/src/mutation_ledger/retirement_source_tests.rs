use super::*;
use crate::mutation_ledger::{lifecycle_tests::lifecycle, tests::Fixture};
use sha2::{Digest, Sha256};

async fn capture(ledger: &Ledger) -> (Context, Vec<SourceRow>) {
    let mut context = None;
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let page = ledger.retirement_inventory(cursor).await.unwrap();
        context.get_or_insert(page.context);
        rows.extend(page.rows);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    (context.unwrap(), rows)
}

async fn live(ledger: &Ledger, fixture: &Fixture) {
    for (i, (operation, receipt)) in lifecycle(&fixture.config, "source-check")
        .into_iter()
        .take(3)
        .enumerate()
    {
        ledger
            .admit(i as u64 + 1, vec![operation.clone()])
            .await
            .unwrap();
        let record = ledger.begin(operation).await.unwrap().record;
        ledger
            .finish(record.completed(&receipt).unwrap())
            .await
            .unwrap();
    }
}

fn digest(rows: &[SourceRow]) -> Vec<u8> {
    let mut digest = Sha256::new();
    for row in rows {
        row.hash_into(&mut digest);
    }
    digest.finalize().to_vec()
}

#[tokio::test]
async fn retirement_source_comparison_preserves_all_five_tables_and_exact_bytes() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    live(&ledger, &fixture).await;
    ledger.storage.call(|db| {
        db.execute("UPDATE operations SET identity=CAST(CAST(identity AS TEXT)||' ' AS BLOB),record=CAST(CAST(record AS TEXT)||' ' AS BLOB)", []).unwrap();
        Ok(())
    }).await.unwrap();
    let (context, rows) = capture(&ledger).await;
    let kinds = rows
        .iter()
        .map(|r| r.ordering().0)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(kinds, [3, 4, 5, 6, 7].into());
    let before = ledger.snapshot().await.unwrap();
    let hash = digest(&rows);
    ledger
        .verify_retirement_sources(context.clone(), rows)
        .await
        .unwrap();
    assert_eq!(ledger.snapshot().await.unwrap(), before);
    let (after, rows) = capture(&ledger).await;
    assert!(after == context);
    assert_eq!(digest(&rows), hash);
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_source_comparison_refuses_raw_column_drift_without_revision_change() {
    for sql in [
        "UPDATE operations SET identity=CAST(CAST(identity AS TEXT)||' ' AS BLOB)",
        "UPDATE operations SET record=CAST(CAST(record AS TEXT)||' ' AS BLOB)",
        "UPDATE operations SET normal=1-normal",
        "UPDATE operations SET status=0,record=NULL",
        "UPDATE authority SET present=CAST(CAST(present AS TEXT)||' ' AS BLOB)",
        "UPDATE authority SET high_water=x'0000000000000009'",
        "UPDATE cleanup_budget SET available=1-available",
        "UPDATE scope_budget SET pending=pending+1",
        "UPDATE logical_processes SET cwd=cwd||'x'",
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        live(&ledger, &fixture).await;
        let (context, rows) = capture(&ledger).await;
        let config = fixture.config.clone();
        let expected = context.clone();
        let context = ledger
            .storage
            .call(move |db| {
                assert!(db.execute(sql, []).unwrap() > 0);
                // Page geometry may change when a raw record shrinks. Keep the
                // post-edit geometry so exact column checks must detect drift
                // even though identity, revision and counters did not change.
                let current = Context::read(db, &config)?;
                assert_eq!(current.store, expected.store);
                assert_eq!(current.generation, expected.generation);
                assert_eq!(current.accounting, expected.accounting);
                Ok(current)
            })
            .await
            .unwrap();
        assert!(
            ledger
                .verify_retirement_sources(context, rows)
                .await
                .is_err(),
            "{sql}"
        );
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_source_comparison_refuses_missing_rows_and_changed_context() {
    for sql in [
        "DELETE FROM operations",
        "DELETE FROM authority",
        "DELETE FROM cleanup_budget",
        "DELETE FROM scope_budget",
        "DELETE FROM logical_processes",
        "UPDATE accounting SET revision=revision+1",
        "UPDATE metadata SET store_id=randomblob(16)",
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        live(&ledger, &fixture).await;
        let (context, rows) = capture(&ledger).await;
        ledger
            .storage
            .call(move |db| {
                assert!(db.execute(sql, []).unwrap() > 0);
                Ok(())
            })
            .await
            .unwrap();
        assert!(
            ledger
                .verify_retirement_sources(context, rows)
                .await
                .is_err(),
            "{sql}"
        );
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_source_comparison_refuses_empty_duplicate_unordered_and_oversized_pages() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    live(&ledger, &fixture).await;
    let (context, rows) = capture(&ledger).await;
    let mut reversed = rows.clone();
    reversed.reverse();
    let mut oversized = rows.clone();
    for row in &mut oversized {
        if let SourceRow::Operation {
            record: Some(bytes),
            ..
        } = row
        {
            bytes.resize(BYTES, b' ');
        }
    }
    for invalid in [
        Vec::new(),
        vec![rows[0].clone(); 2],
        vec![rows[0].clone(); ROWS + 1],
        reversed,
        oversized,
    ] {
        assert!(ledger
            .verify_retirement_sources(context.clone(), invalid)
            .await
            .is_err());
    }
    ledger
        .verify_retirement_sources(context, rows)
        .await
        .unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_source_comparison_rechecks_after_an_unrelated_admitted_effect() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    live(&ledger, &fixture).await;
    let (context, rows) = capture(&ledger).await;
    ledger
        .verify_retirement_sources(context.clone(), rows.clone())
        .await
        .unwrap();
    let operation = lifecycle(&fixture.config, "different").remove(0).0;
    ledger.admit(1, vec![operation]).await.unwrap();
    assert!(ledger
        .verify_retirement_sources(context, rows)
        .await
        .is_err());
    let (context, rows) = capture(&ledger).await;
    ledger
        .verify_retirement_sources(context, rows)
        .await
        .unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_source_query_failure_preserves_database_and_emergency_reserve() {
    for table in ["metadata", "operations"] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        live(&ledger, &fixture).await;
        let (context, rows) = capture(&ledger).await;
        ledger
            .storage
            .call(move |db| {
                // Shadow the source on this connection, causing a real query
                // error without changing the durable schema or page geometry.
                db.execute_batch(&format!("CREATE TEMP TABLE {table} (unavailable INTEGER)"))
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        let database = fixture.database().join("ledger.sqlite3");
        let reserve = fixture.database().join("emergency.reserve");
        let before = std::fs::read(&database).unwrap();
        let reserve_before = std::fs::read(&reserve).unwrap();
        assert_eq!(
            ledger
                .verify_retirement_sources(context, rows.clone())
                .await
                .unwrap_err()
                .code,
            "mutation_ledger_invalid"
        );
        assert_eq!(std::fs::read(&database).unwrap(), before);
        assert_eq!(std::fs::read(&reserve).unwrap(), reserve_before);
        crate::ledger_storage::reserve::require(&fixture.database()).unwrap();
        ledger
            .storage
            .call(move |db| {
                db.execute_batch(&format!("DROP TABLE temp.{table}"))
                    .unwrap();
                Ok(())
            })
            .await
            .unwrap();
        let (context, after) = capture(&ledger).await;
        assert_eq!(digest(&after), digest(&rows));
        ledger
            .verify_retirement_sources(context, after)
            .await
            .unwrap();
        ledger.close().await.unwrap();
        fixture.open(false).await.close().await.unwrap();
    }
}
