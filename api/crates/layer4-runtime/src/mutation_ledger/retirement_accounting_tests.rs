use super::tests::{finish, inventory};
use super::*;
use crate::mutation_ledger::{budget, lifecycle_tests::lifecycle, tests::Fixture};
use layer3_adapter::{AgentCloseRequest, AgentSlot};

async fn mutate(ledger: &Ledger, sql: String) {
    ledger
        .storage
        .call(move |db| {
            db.execute_batch(&sql).unwrap();
            Ok(())
        })
        .await
        .unwrap();
}

async fn refuses(ledger: &Ledger) {
    let mut cursor = None;
    for _ in 0..20 {
        match ledger.retirement_inventory(cursor).await {
            Err(error) => {
                assert_eq!(error.code, "mutation_ledger_invalid");
                return;
            }
            Ok(page) => {
                assert!(page.next.is_some(), "inconsistent accounting was accepted");
                cursor = page.next;
            }
        }
    }
    panic!("fixture exceeded bounded page count");
}

async fn cycle(ledger: &Ledger, config: &RuntimeConfig, count: usize) {
    for (index, (operation, receipt)) in lifecycle(config, "accounting-agent")
        .into_iter()
        .take(count)
        .enumerate()
    {
        ledger
            .admit(index as u64 + 1, vec![operation.clone()])
            .await
            .unwrap();
        let entry = ledger.begin(operation).await.unwrap();
        ledger
            .finish(entry.record.completed(&receipt).unwrap())
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_reconciles_global_counters_and_detects_missing_rows() {
    for sql in [
        format!(
            "UPDATE accounting SET reserved=reserved+{}",
            budget::OUTCOME
        ),
        "UPDATE accounting SET operations=operations+1".into(),
        "UPDATE accounting SET authority=authority+1".into(),
        "UPDATE accounting SET terminal=terminal-1".into(),
        "DELETE FROM operations".into(),
        "DELETE FROM authority".into(),
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        finish(&ledger, 0).await;
        inventory(&ledger).await;
        mutate(&ledger, sql).await;
        let before = ledger.snapshot().await.unwrap();
        refuses(&ledger).await;
        assert_eq!(ledger.snapshot().await.unwrap(), before);
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_reconciles_available_and_consumed_cleanup_reservations() {
    for live in [false, true] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        cycle(&ledger, &fixture.config, if live { 3 } else { 6 }).await;
        inventory(&ledger).await;
        let sql = if live {
            "DELETE FROM cleanup_budget WHERE resource=(SELECT resource FROM cleanup_budget ORDER BY scope,resource LIMIT 1)"
        } else {
            "UPDATE cleanup_budget SET available=1 WHERE resource=(SELECT resource FROM cleanup_budget ORDER BY scope,resource LIMIT 1)"
        };
        mutate(&ledger, sql.into()).await;
        refuses(&ledger).await;
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_reconciles_covered_outstanding_work_without_normal_budget() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    cycle(&ledger, &fixture.config, 3).await;
    let (operation, receipt) = lifecycle(&fixture.config, "accounting-agent")[3].clone();
    ledger.admit(4, vec![operation.clone()]).await.unwrap();
    let entry = ledger.begin(operation.clone()).await.unwrap();
    let before = ledger.snapshot().await.unwrap();
    assert_eq!(before.pending, 0);
    assert_eq!(before.operations - before.terminal, 1);
    assert_eq!(before.reserved_bytes, 3 * budget::OUTCOME);
    inventory(&ledger).await;
    let id = operation.operation_id;
    ledger
        .storage
        .call(move |db| {
            db.execute("UPDATE operations SET normal=1 WHERE operation_id=?", [id])
                .unwrap();
            Ok(())
        })
        .await
        .unwrap();
    refuses(&ledger).await;
    mutate(
        &ledger,
        "UPDATE operations SET normal=0 WHERE status=1".into(),
    )
    .await;
    ledger
        .finish(entry.record.completed(&receipt).unwrap())
        .await
        .unwrap();
    inventory(&ledger).await;
    assert_eq!(
        ledger.snapshot().await.unwrap().reserved_bytes,
        2 * budget::OUTCOME
    );
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn retirement_inventory_refuses_scope_counter_swaps_even_when_global_totals_match() {
    for processes in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.home_identity = Some("accounting-home".into());
        fixture.config.generation = 1;
        let ledger = fixture.open(true).await;
        if processes {
            cycle(&ledger, &fixture.config, 3).await;
        } else {
            let op = Operation::close_agent(&AgentCloseRequest::new(
                "agent:accounting-agent:close:v1",
                AgentSlot::new("accounting-agent"),
            ));
            ledger.admit(1, vec![op]).await.unwrap();
        }
        ledger.close().await.unwrap();
        fixture.config.generation = 2;
        let ledger = fixture.open(false).await;
        let operation = if processes {
            lifecycle(&fixture.config, "accounting-agent")[2].0.clone()
        } else {
            Operation::close_agent(&AgentCloseRequest::new(
                "agent:accounting-agent:close:v1",
                AgentSlot::new("accounting-agent"),
            ))
        };
        ledger.admit(1, vec![operation.clone()]).await.unwrap();
        if processes {
            let entry = ledger.begin(operation).await.unwrap();
            let receipt = lifecycle(&fixture.config, "accounting-agent")[2].1.clone();
            ledger
                .finish(entry.record.completed(&receipt).unwrap())
                .await
                .unwrap();
        }
        inventory(&ledger).await;
        // Preserve the global total while charging both generations to the new
        // scope. Aggregate-only verification would accept this corruption.
        let column = if processes { "processes" } else { "pending" };
        mutate(&ledger, format!(
            "UPDATE scope_budget SET {column}=CASE scope WHEN x'0000000000000001' THEN 0 ELSE 2 END WHERE scope IN (x'0000000000000001',x'0000000000000002')"
        )).await;
        refuses(&ledger).await;
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_refuses_missing_or_orphan_nonzero_scope_budget() {
    for missing in [false, true] {
        let mut fixture = Fixture::new();
        fixture.config.home_identity = Some("accounting-home".into());
        fixture.config.generation = 2;
        let ledger = fixture.open(true).await;
        let operation = Operation::close_agent(&AgentCloseRequest::new(
            "agent:accounting-agent:close:v1",
            AgentSlot::new("accounting-agent"),
        ));
        ledger.admit(1, vec![operation]).await.unwrap();
        inventory(&ledger).await;
        mutate(
            &ledger,
            if missing {
                "DELETE FROM scope_budget".into()
            } else {
                "INSERT INTO scope_budget VALUES (x'0000000000000001',1,0)".into()
            },
        )
        .await;
        refuses(&ledger).await;
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn retirement_inventory_reconciles_process_projection_counts_without_mutating_them() {
    for sql in [
        "DELETE FROM logical_processes",
        "UPDATE scope_budget SET processes=processes+1",
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        cycle(&ledger, &fixture.config, 3).await;
        inventory(&ledger).await;
        mutate(&ledger, sql.into()).await;
        let before = ledger.snapshot().await.unwrap();
        refuses(&ledger).await;
        assert_eq!(ledger.snapshot().await.unwrap(), before);
        ledger.close().await.unwrap();
    }
}
