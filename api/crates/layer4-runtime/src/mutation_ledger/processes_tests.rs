use super::tests::{close, Fixture};
use super::*;
use crate::mutation_journal::{Status, Subject};
use layer3_adapter::wire::{AgentCloseReceipt, AgentOpenReceipt};
use layer3_adapter::AgentSlot;

fn open(config: &RuntimeConfig) -> Operation {
    Operation {
        operation_id: "agent:agent:open:v1".into(),
        request_sha256: "a".repeat(64),
        payload_sha256: "b".repeat(64),
        subject: Subject::OpenAgent {
            agent_id: "agent".into(),
            cwd: crate::worktree::expected_path(&config.home, "workspace", "agent")
                .to_string_lossy()
                .into(),
            home: config.agent_home("agent").to_string_lossy().into(),
        },
    }
}
async fn count(ledger: &Ledger) -> i64 {
    let scope = ledger
        .config
        .home_identity
        .as_ref()
        .map_or(0, |_| ledger.config.generation)
        .to_be_bytes();
    ledger
        .storage
        .call(move |db| {
            Ok(db
                .query_row(
                    "SELECT count(*) FROM logical_processes WHERE scope=?",
                    [scope.as_slice()],
                    |r| r.get(0),
                )
                .unwrap())
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn process_projection_keeps_started_completed_and_ambiguous_until_completed_close() {
    for status in [
        Status::Started,
        Status::Completed,
        Status::Failed,
        Status::Indeterminate,
    ] {
        let fixture = Fixture::new();
        let ledger = fixture.open(true).await;
        let operation = open(&fixture.config);
        ledger.admit(1, vec![operation.clone()]).await.unwrap();
        assert_eq!(count(&ledger).await, 0); // Reservation alone is not a launched process.
        let record = ledger.begin(operation.clone()).await.unwrap().record;
        assert_eq!(count(&ledger).await, 1);
        let Subject::OpenAgent { cwd, home, .. } = &operation.subject else {
            unreachable!()
        };
        let terminal = match status {
            Status::Started => None,
            Status::Completed => Some(
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
            ),
            Status::Failed => Some(
                record
                    .failed(
                        &RuntimeError::rejected("spawn_refused", "spawn refused"),
                        false,
                    )
                    .unwrap(),
            ),
            Status::Indeterminate => Some(
                record
                    .failed(
                        &RuntimeError::rejected("agent_open_outcome_unknown", "unproved spawn"),
                        true,
                    )
                    .unwrap(),
            ),
        };
        if let Some(record) = terminal {
            ledger.finish(record).await.unwrap();
        }
        assert_eq!(count(&ledger).await, i64::from(status != Status::Failed));
        ledger.admit(2, vec![close()]).await.unwrap();
        assert_eq!(count(&ledger).await, i64::from(status != Status::Failed));
        let close_record = ledger.begin(close()).await.unwrap().record;
        assert_eq!(count(&ledger).await, i64::from(status != Status::Failed));
        ledger
            .finish(
                close_record
                    .completed(&AgentCloseReceipt {
                        operation_id: close().operation_id,
                        request_sha256: close().request_sha256,
                        agent_id: AgentSlot::new("agent"),
                    })
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(count(&ledger).await, 0);
        ledger.close().await.unwrap();
        let ledger = fixture.open(false).await;
        assert_eq!(count(&ledger).await, 0);
        ledger.close().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_storage_failure_rolls_back_projection_and_outcome_together() {
    let fixture = Fixture::new();
    let ledger = fixture.open(true).await;
    let operation = open(&fixture.config);
    ledger.admit(1, vec![operation.clone()]).await.unwrap();
    ledger.begin(operation.clone()).await.unwrap();
    ledger.admit(2, vec![close()]).await.unwrap();
    let terminal = ledger
        .begin(close())
        .await
        .unwrap()
        .record
        .completed(&AgentCloseReceipt {
            operation_id: close().operation_id,
            request_sha256: close().request_sha256,
            agent_id: AgentSlot::new("agent"),
        })
        .unwrap();
    ledger.storage.call(|db| {
        db.execute_batch("CREATE TEMP TRIGGER fail_accounting BEFORE UPDATE ON accounting BEGIN SELECT RAISE(ABORT, 'injected'); END").unwrap();
        Ok(())
    }).await.unwrap();
    assert_eq!(
        ledger.finish(terminal.clone()).await.unwrap_err().code,
        "mutation_ledger_storage_hold"
    );
    assert_eq!(count(&ledger).await, 1);
    ledger
        .storage
        .call(|db| {
            db.execute_batch("DROP TRIGGER fail_accounting").unwrap();
            Ok(())
        })
        .await
        .unwrap();
    assert_eq!(
        ledger.begin(close()).await.unwrap().record.status(),
        Status::Started
    );
    ledger.close().await.unwrap();
    let ledger = fixture.open(false).await;
    assert_eq!(count(&ledger).await, 1);
    assert_eq!(
        ledger.snapshot().await.unwrap().emergency_remaining_bytes,
        0
    );
    let fresh = Operation::close_agent(&layer3_adapter::AgentCloseRequest::new(
        "agent:new:close:v1",
        AgentSlot::new("new"),
    ));
    assert_eq!(
        ledger.admit(1, vec![fresh]).await.unwrap_err().code,
        "mutation_ledger_storage_hold"
    );
    ledger.finish(terminal).await.unwrap();
    assert_eq!(count(&ledger).await, 0);
    ledger.close().await.unwrap();
}

#[tokio::test]
async fn process_projection_and_receipts_cannot_supply_a_later_generation() {
    let mut fixture = Fixture::new();
    fixture.config.home_identity = Some("a".repeat(64));
    let ledger = fixture.open(true).await;
    let operation = open(&fixture.config);
    ledger.admit(1, vec![operation.clone()]).await.unwrap();
    ledger.begin(operation.clone()).await.unwrap();
    assert_eq!(count(&ledger).await, 1);
    ledger.close().await.unwrap();
    fixture.config.generation = 2;
    let ledger = fixture.open(false).await;
    assert_eq!(count(&ledger).await, 0);
    assert!(ledger.begin(operation.clone()).await.is_err());
    ledger.admit(1, vec![operation.clone()]).await.unwrap();
    assert!(ledger.begin(operation).await.unwrap().new);
    assert_eq!(count(&ledger).await, 1);
    ledger.close().await.unwrap();
}
