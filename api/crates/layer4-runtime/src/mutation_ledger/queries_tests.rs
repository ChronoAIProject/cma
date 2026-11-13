use super::tests::{add, close, remove, Fixture};
use super::*;
use crate::mutation_journal::{Record, Status};
use layer3_adapter::{wire::AgentCloseReceipt, AgentCloseRequest, AgentSlot};

fn named(name: &str) -> Operation {
    Operation::close_agent(&AgentCloseRequest::new(
        format!("agent:{name}:close:v1"),
        AgentSlot::new(name),
    ))
}
fn receipt(operation: &Operation) -> AgentCloseReceipt {
    let crate::mutation_journal::Subject::CloseAgent { agent_id } = &operation.subject else {
        panic!("close")
    };
    AgentCloseReceipt {
        operation_id: operation.operation_id.clone(),
        request_sha256: operation.request_sha256.clone(),
        agent_id: AgentSlot::new(agent_id),
    }
}

#[tokio::test]
async fn newer_cleanup_is_authority_before_any_started_create_is_recovered() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    ledger.admit(1, vec![add(0)]).await.unwrap();
    assert!(!ledger.admitted(add(0)).await.unwrap());
    ledger.begin(add(0)).await.unwrap();
    assert!(ledger.admitted(add(0)).await.unwrap());
    ledger.admit(2, vec![remove()]).await.unwrap();
    assert!(!ledger.admitted(remove()).await.unwrap());
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    let started = ledger.started_page(0, String::new()).await.unwrap();
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].operation(), add(0));
    assert_eq!(ledger.recovery_fence(add(0)).await.unwrap(), None);
    assert_eq!(ledger.recovery_fence(remove()).await.unwrap(), Some(2));
    ledger
        .finish(
            started[0]
                .clone()
                .failed(
                    &RuntimeError::rejected(
                        "mutation_authority_superseded",
                        "runtime mutation was superseded by newer resource authority",
                    ),
                    false,
                )
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(ledger
        .started_page(0, String::new())
        .await
        .unwrap()
        .is_empty());
    assert!(!ledger.admitted(remove()).await.unwrap()); // Never promote authority-only recovery.
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn ten_thousand_resources_leave_only_started_work_in_bounded_indexed_recovery() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    for i in 0..10_000 {
        let operation = named(&format!("history-{i:05}"));
        ledger.admit(1, vec![operation.clone()]).await.unwrap();
        let record = ledger.begin(operation.clone()).await.unwrap().record;
        ledger
            .finish(record.completed(&receipt(&operation)).unwrap())
            .await
            .unwrap();
    }
    for i in 0..3 {
        let operation = named(&format!("pending-{i:05}"));
        ledger.admit(1, vec![operation.clone()]).await.unwrap();
        ledger.begin(operation).await.unwrap();
    }
    let page = ledger.started_page(0, String::new()).await.unwrap();
    assert_eq!(page.len(), 3);
    assert!(page
        .iter()
        .all(|r| r.operation().operation_id.contains("pending-")));
    let snapshot = ledger.snapshot().await.unwrap();
    assert_eq!(
        (
            snapshot.operations,
            snapshot.terminal,
            snapshot.pending,
            snapshot.reserved_bytes
        ),
        (10_003, 10_000, 3, 3 * budget::OUTCOME)
    );
    let first = named("history-00000");
    ledger.admit(u64::MAX, vec![first.clone()]).await.unwrap();
    assert_eq!(
        ledger
            .begin(first.clone())
            .await
            .unwrap()
            .record
            .receipt::<AgentCloseReceipt>()
            .unwrap(),
        receipt(&first)
    );
    let bytes = std::fs::read(fixture.database().join("ledger.sqlite3")).unwrap();
    let snapshot = ledger.snapshot().await.unwrap();
    ledger.active(ProcessFilter::Any).await.unwrap();
    ledger.started_page(0, String::new()).await.unwrap();
    ledger.admitted(first.clone()).await.unwrap();
    ledger.recovery_fence(first).await.unwrap();
    assert_eq!(ledger.snapshot().await.unwrap(), snapshot);
    assert_eq!(
        std::fs::read(fixture.database().join("ledger.sqlite3")).unwrap(),
        bytes
    );
    ledger
        .storage
        .call(|db| {
            let plan = db
                .prepare(&format!("EXPLAIN QUERY PLAN {}", queries::STARTED_PAGE))
                .unwrap()
                .query_map(rusqlite::params![[0u8; 8].as_slice(), ""], |r| {
                    r.get::<_, String>(3)
                })
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
                .join(";");
            assert!(
                plan.contains("SEARCH operations USING INDEX outstanding"),
                "{plan}"
            );
            assert!(!plan.contains("SCAN"), "{plan}");
            Ok(())
        })
        .await
        .unwrap();
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn recovery_pages_are_keyset_bounded_and_generation_checked() {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("a".repeat(64));
    let ledger = Ledger::open(fixture.database(), fixture.config.clone(), true, 1_048_576)
        .await
        .unwrap();
    for i in 0..130 {
        let operation = named(&format!("pending-{i:05}"));
        ledger.admit(1, vec![operation.clone()]).await.unwrap();
        ledger.begin(operation).await.unwrap();
    }
    assert!(ledger
        .started_page(0, String::new())
        .await
        .unwrap()
        .is_empty());
    let mut cursor = String::new();
    let mut sizes = Vec::new();
    loop {
        let page = ledger.started_page(1, cursor).await.unwrap();
        if page.is_empty() {
            break;
        }
        assert!(page.len() <= 64);
        sizes.push(page.len());
        cursor = page.last().unwrap().operation().operation_id;
    }
    assert_eq!(sizes, [64, 64, 2]);
    assert!(ledger.started_page(2, String::new()).await.is_err());
    assert!(ledger.started_page(1, "a".repeat(257)).await.is_err());
    ledger.close().await.unwrap();
    fixture.config.generation = 2;
    let ledger = Ledger::open(fixture.database(), fixture.config.clone(), false, 1_048_576)
        .await
        .unwrap();
    assert!(ledger
        .started_page(2, String::new())
        .await
        .unwrap()
        .is_empty());
    assert_eq!(ledger.snapshot().await.unwrap().pending, 0);
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn ten_thousand_fences_keep_one_operation_and_exact_terminal_bytes() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    ledger.admit(1, vec![close()]).await.unwrap();
    let record = ledger
        .begin(close())
        .await
        .unwrap()
        .record
        .completed(&receipt(&close()))
        .unwrap();
    ledger.finish(record.clone()).await.unwrap();
    for fence in 2..=10_000 {
        ledger.admit(fence, vec![close()]).await.unwrap();
    }
    let snapshot = ledger.snapshot().await.unwrap();
    assert_eq!(
        (
            snapshot.operations,
            snapshot.terminal,
            snapshot.authority,
            snapshot.reserved_bytes
        ),
        (1, 1, 1, 0)
    );
    let bytes = ledger
        .begin(close())
        .await
        .unwrap()
        .record
        .encode()
        .unwrap();
    assert_eq!(bytes, record.encode().unwrap());
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    assert_eq!(ledger.recovery_fence(close()).await.unwrap(), Some(10_000));
    assert_eq!(
        ledger.admit(9999, vec![close()]).await.unwrap_err().code,
        "mutation_authority_stale"
    );
    assert_eq!(
        Record::decode(&bytes, &fixture.config).unwrap().status(),
        Status::Completed
    );
    ledger.close().await.unwrap();
}
