use super::processes_tests::open;
use super::tests::{close, Fixture};
use super::*;
use crate::mutation_journal::{Record, Status, Subject};
use layer3_adapter::wire::{AgentCloseReceipt, AgentOpenReceipt};
use layer3_adapter::{AgentCloseRequest, AgentSlot};

async fn rows(ledger: &Ledger) -> Vec<(Vec<u8>, i64, i64)> {
    ledger
        .storage
        .call(|db| {
            let mut query = db
                .prepare("SELECT scope,pending,processes FROM scope_budget ORDER BY scope")
                .unwrap();
            Ok(query
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap())
        })
        .await
        .unwrap()
}

fn terminal(record: Record, operation: &Operation, agent: &str) -> Record {
    record
        .completed(&AgentCloseReceipt {
            operation_id: operation.operation_id.clone(),
            request_sha256: operation.request_sha256.clone(),
            agent_id: AgentSlot::new(agent),
        })
        .unwrap()
}

#[tokio::test]
async fn settled_generation_budgets_stay_empty_across_both_legacy_history_bounds() {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("a".repeat(43));
    for generation in 1..=1025 {
        fixture.config.generation = generation;
        let ledger = fixture.open(generation == 1).await;
        for slot in 0..4 {
            let name = format!("slot-{slot}");
            let operation = Operation::close_agent(&AgentCloseRequest::new(
                format!("agent:{name}:close:v1"),
                AgentSlot::new(&name),
            ));
            ledger.admit(1, vec![operation.clone()]).await.unwrap();
            let record = ledger.begin(operation.clone()).await.unwrap().record;
            ledger
                .finish(terminal(record, &operation, &name))
                .await
                .unwrap();
        }
        assert!(rows(&ledger).await.is_empty());
        if matches!(generation, 256 | 257 | 1024 | 1025) {
            let state = ledger.snapshot().await.unwrap();
            assert_eq!(
                (state.operations, state.terminal, state.authority),
                (
                    generation as i64 * 4,
                    generation as i64 * 4,
                    generation as i64 * 4
                )
            );
            assert_eq!(
                (state.pending, state.processes, state.reserved_bytes),
                (0, 0, 0)
            );
        }
        ledger.close().await.unwrap();
    }
    let ledger = fixture.open(false).await;
    assert!(rows(&ledger).await.is_empty());
    let operation = Operation::close_agent(&AgentCloseRequest::new(
        "agent:slot-0:close:v1",
        AgentSlot::new("slot-0"),
    ));
    let before = ledger.snapshot().await.unwrap();
    ledger
        .admit(u64::MAX, vec![operation.clone()])
        .await
        .unwrap();
    let replay = ledger.begin(operation).await.unwrap();
    assert!(!replay.new);
    assert_eq!(replay.record.status(), Status::Completed);
    assert_eq!(
        replay
            .record
            .receipt::<AgentCloseReceipt>()
            .unwrap()
            .agent_id,
        AgentSlot::new("slot-0")
    );
    let after = ledger.snapshot().await.unwrap();
    assert_eq!(
        (
            after.operations,
            after.terminal,
            after.authority,
            after.reserved_bytes
        ),
        (
            before.operations,
            before.terminal,
            before.authority,
            before.reserved_bytes
        )
    );
    assert!(rows(&ledger).await.is_empty());
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn empty_budget_pruning_keeps_live_processes_and_other_pending_work() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    let operation = open(&fixture.config);
    ledger.admit(1, vec![operation.clone()]).await.unwrap();
    let record = ledger.begin(operation.clone()).await.unwrap().record;
    let Subject::OpenAgent { cwd, home, .. } = &operation.subject else {
        panic!()
    };
    ledger
        .finish(
            record
                .completed(&AgentOpenReceipt {
                    operation_id: operation.operation_id.clone(),
                    request_sha256: operation.request_sha256.clone(),
                    agent_id: AgentSlot::new("agent"),
                    cwd: cwd.clone(),
                    home: home.clone(),
                    conversation_id: None,
                })
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        rows(&ledger).await,
        vec![(0u64.to_be_bytes().to_vec(), 0, 1)]
    );
    let other = Operation::close_agent(&AgentCloseRequest::new(
        "agent:other:close:v1",
        AgentSlot::new("other"),
    ));
    ledger.admit(2, vec![close(), other.clone()]).await.unwrap();
    let record = ledger.begin(close()).await.unwrap().record;
    ledger
        .finish(terminal(record, &close(), "agent"))
        .await
        .unwrap();
    assert_eq!(
        rows(&ledger).await,
        vec![(0u64.to_be_bytes().to_vec(), 1, 0)]
    );
    let record = ledger.begin(other.clone()).await.unwrap().record;
    ledger
        .finish(terminal(record, &other, "other"))
        .await
        .unwrap();
    assert!(rows(&ledger).await.is_empty());
    assert_eq!(ledger.snapshot().await.unwrap().operations, 3);
    assert_eq!(
        ledger
            .admit(u64::MAX, vec![operation])
            .await
            .unwrap_err()
            .code,
        "mutation_authority_retired"
    );
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn failed_budget_pruning_rolls_back_the_receipt_and_counters_then_recovers() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    ledger.admit(1, vec![close()]).await.unwrap();
    let record = ledger.begin(close()).await.unwrap().record;
    let completed = terminal(record, &close(), "agent");
    let before = ledger.snapshot().await.unwrap();
    let budget = rows(&ledger).await;
    ledger.storage.call(|db| {
        db.execute_batch("CREATE TEMP TRIGGER fail_scope_prune BEFORE DELETE ON scope_budget BEGIN SELECT RAISE(ABORT, 'injected'); END").unwrap();
        Ok(())
    }).await.unwrap();
    assert_eq!(
        ledger.finish(completed.clone()).await.unwrap_err().code,
        "mutation_ledger_storage_hold"
    );
    assert_eq!(rows(&ledger).await, budget);
    assert_eq!(
        ledger.test_record(close()).await.unwrap().status(),
        Status::Started
    );
    let after = ledger.snapshot().await.unwrap();
    assert_eq!(
        (
            after.revision,
            after.operations,
            after.terminal,
            after.authority,
            after.reserved_bytes,
            after.pending
        ),
        (
            before.revision,
            before.operations,
            before.terminal,
            before.authority,
            before.reserved_bytes,
            before.pending
        )
    );
    assert_eq!(after.emergency_remaining_bytes, 0);
    ledger
        .storage
        .call(|db| {
            db.execute_batch("DROP TRIGGER fail_scope_prune").unwrap();
            Ok(())
        })
        .await
        .unwrap();
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    assert_eq!(rows(&ledger).await, budget);
    ledger.finish(completed).await.unwrap();
    assert!(rows(&ledger).await.is_empty());
    let replay = ledger.begin(close()).await.unwrap();
    assert!(!replay.new);
    assert_eq!(replay.record.status(), Status::Completed);
    ledger.close().await.unwrap();
}
