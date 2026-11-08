use super::tests::{add, close, remove, Fixture};
use super::*;
use crate::mutation_journal::{Record, Status, Subject};
use layer3_adapter::{
    wire::AgentCloseReceipt, AgentCloseRequest, AgentSlot, WorktreeRemoveReceipt,
};

fn close_named(name: &str) -> Operation {
    Operation::close_agent(&AgentCloseRequest::new(
        format!("agent:{name}:close:v1"),
        AgentSlot::new(name),
    ))
}
fn receipt(operation: &Operation) -> AgentCloseReceipt {
    let Subject::CloseAgent { agent_id } = &operation.subject else {
        panic!("close")
    };
    AgentCloseReceipt {
        operation_id: operation.operation_id.clone(),
        request_sha256: operation.request_sha256.clone(),
        agent_id: AgentSlot::new(agent_id),
    }
}
async fn counts(ledger: &Ledger) -> (i64, i64, i64, i64) {
    ledger
        .storage
        .call(|db| {
            Ok(db
                .query_row(
                    "SELECT reserved,operations,terminal,authority FROM accounting",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap())
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn admission_start_outcome_and_exact_replay_are_distinct_durable_boundaries() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    assert!(ledger.begin(close()).await.is_err());
    ledger.admit(1, vec![close()]).await.unwrap();
    assert_eq!(counts(&ledger).await, (budget::OUTCOME, 1, 0, 1));
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    let entry = ledger.begin(close()).await.unwrap();
    assert!(entry.new);
    assert_eq!(entry.record.status(), Status::Started);
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    let entry = ledger.begin(close()).await.unwrap();
    assert!(!entry.new);
    let terminal = entry.record.completed(&receipt(&close())).unwrap();
    ledger.finish(terminal.clone()).await.unwrap();
    ledger.finish(terminal).await.unwrap(); // Lost reply: counters do not move twice.
    assert_eq!(counts(&ledger).await, (0, 1, 1, 1));
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    ledger.admit(2, vec![close()]).await.unwrap();
    let replay = ledger.begin(close()).await.unwrap();
    assert_eq!(
        replay.record.receipt::<AgentCloseReceipt>().unwrap(),
        receipt(&close())
    );
    let failed = Record::started(&fixture.config, &close())
        .unwrap()
        .failed(&RuntimeError::rejected("different", "different"), false)
        .unwrap();
    assert!(ledger.finish(failed).await.is_err());
    assert_eq!(counts(&ledger).await, (0, 1, 1, 1));
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn normal_byte_refusal_preserves_replay_and_each_existing_cleanup_reservation() {
    let fixture = Fixture::new();
    let ledger = Ledger::open(fixture.database(), fixture.config.clone(), true, 16_384)
        .await
        .unwrap();
    ledger.admit(1, vec![add(0)]).await.unwrap();
    let mut accepted = Vec::new();
    for i in 0..100 {
        let operation = close_named(&format!("extra-{i}"));
        match ledger.admit(1, vec![operation.clone()]).await {
            Ok(()) => accepted.push(operation),
            Err(error) => {
                assert_eq!(error.code, "mutation_ledger_capacity");
                assert!(!error.retryable);
                break;
            }
        }
    }
    assert!(!accepted.is_empty() && accepted.len() < 100);
    let before = counts(&ledger).await;
    assert_eq!(
        ledger
            .admit(1, vec![close_named("unknown")])
            .await
            .unwrap_err()
            .code,
        "mutation_ledger_capacity"
    );
    assert_eq!(counts(&ledger).await, before);
    ledger.admit(2, vec![remove()]).await.unwrap();
    let entry = ledger.begin(remove()).await.unwrap();
    ledger
        .finish(
            entry
                .record
                .completed(&WorktreeRemoveReceipt {
                    operation_id: remove().operation_id,
                    request_sha256: remove().request_sha256,
                    workspace: "workspace".into(),
                    name: "agent".into(),
                })
                .unwrap(),
        )
        .await
        .unwrap();
    ledger.admit(3, vec![remove()]).await.unwrap();
    assert_eq!(
        ledger.begin(remove()).await.unwrap().record.status(),
        Status::Completed
    );
    for operation in accepted {
        let entry = ledger.begin(operation.clone()).await.unwrap();
        ledger
            .finish(entry.record.completed(&receipt(&operation)).unwrap())
            .await
            .unwrap();
    }
    ledger
        .admit(1, vec![close_named("after-completion")])
        .await
        .unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn pair_capacity_refusal_rolls_back_authority_identity_and_reservations() {
    let fixture = Fixture::new();
    // Shared headroom + just one new unknown-cleanup outcome, never two.
    let pages = ((budget::HEADROOM + budget::OUTCOME + 1024 * 1024) / 4096) as u32;
    let ledger = Ledger::open(fixture.database(), fixture.config.clone(), true, pages)
        .await
        .unwrap();
    let before = counts(&ledger).await;
    assert_eq!(
        ledger
            .admit(9, vec![close(), close_named("second")])
            .await
            .unwrap_err()
            .code,
        "mutation_ledger_capacity"
    );
    assert_eq!(counts(&ledger).await, before);
    ledger.admit(1, vec![close()]).await.unwrap();
    assert!(ledger.begin(close()).await.unwrap().new);
    assert!(ledger.begin(close_named("second")).await.is_err());
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn failures_and_indeterminate_outcomes_keep_exact_error_and_reject_bad_terminals() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    for (name, indeterminate) in [("failed", false), ("ambiguous", true)] {
        let operation = close_named(name);
        ledger.admit(1, vec![operation.clone()]).await.unwrap();
        let entry = ledger.begin(operation.clone()).await.unwrap();
        assert!(entry
            .record
            .clone()
            .failed(&RuntimeError::unavailable("retry", "retry"), false)
            .is_err());
        let code = if indeterminate {
            "agent_close_outcome_unknown"
        } else {
            "close_refused"
        };
        let error = RuntimeError::rejected(code, "exact retained explanation");
        ledger
            .finish(entry.record.failed(&error, indeterminate).unwrap())
            .await
            .unwrap();
        ledger
            .admit(u64::MAX, vec![operation.clone()])
            .await
            .unwrap();
        let replay = ledger
            .begin(operation)
            .await
            .unwrap()
            .record
            .error()
            .unwrap();
        assert_eq!(
            (replay.code, replay.detail, replay.retryable),
            (error.code, error.detail, false)
        );
    }
    ledger.admit(1, vec![close()]).await.unwrap();
    let entry = ledger.begin(close()).await.unwrap();
    let malformed = entry
        .record
        .completed(&serde_json::json!({"wrong":true}))
        .unwrap();
    assert!(ledger.finish(malformed).await.is_err());
    assert_eq!(
        ledger.begin(close()).await.unwrap().record.status(),
        Status::Started
    );
    ledger
        .storage
        .call(|db| {
            assert!(db
                .execute("UPDATE operations SET status=2 WHERE status=1", [])
                .is_err());
            assert!(db
                .execute("UPDATE operations SET status=0 WHERE status=1", [])
                .is_err());
            assert!(db
                .execute("UPDATE operations SET record=x'7b' WHERE status=1", [])
                .is_err());
            Ok(())
        })
        .await
        .unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn outstanding_1024_to_1025_refuses_before_admission_then_terminal_frees_slot() {
    let fixture = Fixture::new();
    // Logical quota only; SQLite allocates a few MiB, not this maximum.
    let ledger = Ledger::open(fixture.database(), fixture.config.clone(), true, 4_194_304)
        .await
        .unwrap();
    for i in 0..1024 {
        ledger
            .admit(1, vec![close_named(&format!("entry-{i}"))])
            .await
            .unwrap();
    }
    let before = counts(&ledger).await;
    assert_eq!(
        ledger.admit(8, vec![close()]).await.unwrap_err().code,
        "mutation_ledger_capacity"
    );
    assert_eq!(counts(&ledger).await, before);
    assert!(ledger.begin(close()).await.is_err());
    let first = close_named("entry-0");
    let entry = ledger.begin(first.clone()).await.unwrap();
    ledger
        .finish(entry.record.completed(&receipt(&first)).unwrap())
        .await
        .unwrap();
    ledger.admit(1, vec![close()]).await.unwrap();
    assert!(ledger.begin(close()).await.unwrap().new);
    ledger.close().await.unwrap();
}
