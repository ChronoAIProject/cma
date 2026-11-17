use super::*;
use layer2_core::sandbox::store::{
    plan_agent_runtime_ledger_hold_retry, plan_agent_runtime_startup_hold_retry,
    AGENT_RUNTIME_LEDGER_HOLD, AGENT_RUNTIME_STARTUP_HOLD, AGENT_RUNTIME_STARTUP_HOLD_DETAIL,
};

const THIRD: &str = "2026-09-05T00:03:00Z";
const FOURTH: &str = "2026-09-05T00:04:00Z";

async fn full_history() -> Fixture {
    let mut f = Fixture::new(100).await;
    let kind = retry(&f.agent).agent_logs.remove(0);
    let rows: Vec<_> = (f.agent.conversation.last_seq + 1..=10_000)
        .map(|seq| {
            log_document(&AgentLogEntry {
                agent_id: f.agent.id.clone(),
                seq,
                appended_at: MINUTE.into(),
                kind: kind.clone(),
            })
            .unwrap()
        })
        .collect();
    for batch in rows.chunks(200) {
        f.store.agent_log.insert_many(batch).await.unwrap();
    }
    f.agent.conversation.last_seq = 10_000;
    f.agent.revision += 1;
    let mut fields = agent_replacement_fields(&f.agent).unwrap();
    fields.insert("last_seq", 10_000_i64);
    f.change(doc! {"$set": fields, "$unset": {INTEGRITY_FIELD: ""}})
        .await;
    assert_eq!(
        f.store.commit_agent_work(retry(&f.agent)).await,
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    );
    f
}

async fn proposal(f: &Fixture, at: &str, due: &str, ledger: bool) -> AgentWorkMutation {
    let tail = f
        .store
        .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .unwrap();
    let plan = if ledger {
        plan_agent_runtime_ledger_hold_retry
    } else {
        plan_agent_runtime_startup_hold_retry
    };
    plan(
        &f.agent,
        tail.as_ref(),
        "worker",
        f.agent.work.lease.as_ref().unwrap().fence,
        at,
        due,
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn startup_hold_reserves_one_tail_reuses_metadata_and_appends_after_ledger_hold() {
    for bounded in [false, true] {
        for ledger_first in [false, true] {
            let mut f = full_history().await;
            f.store = f.store.with_bounded_close_retries(bounded);
            let original = f.agent.clone();
            let (at, due, next_due) = if ledger_first {
                let ledger = proposal(&f, MINUTE, TWO_MINUTES, true).await;
                assert_eq!(ledger.agent_logs.len(), 1);
                f.store.commit_agent_work(ledger).await.unwrap();
                f.agent = claim(&f.store, &f.agent.id, TWO_MINUTES).await;
                let tail = f
                    .store
                    .agent_log_entry(&f.agent.id, 10_001)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    matches!(tail.kind, AgentLogKind::WorkStepFailed { failure, .. }
                    if failure.class == AGENT_RUNTIME_LEDGER_HOLD)
                );
                (TWO_MINUTES, THIRD, FOURTH)
            } else {
                (MINUTE, TWO_MINUTES, THIRD)
            };
            let mutation = proposal(&f, at, due, false).await;
            assert_eq!(mutation.agent_logs.len(), 1);
            let head = 10_001 + u64::from(ledger_first);
            assert_eq!(mutation.agent.conversation.last_seq, head);
            f.store.commit_agent_work(mutation).await.unwrap();
            f.agent = claim(&f.store, &f.agent.id, due).await;
            let marker = f
                .store
                .agent_log_entry(&f.agent.id, head)
                .await
                .unwrap()
                .unwrap();
            assert!(
                matches!(&marker.kind, AgentLogKind::WorkStepFailed { step, failure }
                if *step == original.work.step && failure.class == AGENT_RUNTIME_STARTUP_HOLD
                    && failure.detail == AGENT_RUNTIME_STARTUP_HOLD_DETAIL)
            );

            let retry = proposal(&f, due, next_due, false).await;
            assert!(retry.agent_logs.is_empty());
            assert_eq!(retry.agent.conversation.last_seq, head);
            let expected = retry.agent.clone();
            f.store.commit_agent_work(retry).await.unwrap();
            assert_eq!(
                f.store.get_agent(&f.agent.id).await.unwrap(),
                Some(expected.clone())
            );
            assert_eq!(expected.status, original.status);
            assert_eq!(expected.work.step, original.work.step);
            assert_eq!(expected.work.receipts, original.work.receipts);
            assert_eq!(
                expected.work.attempts,
                original.work.attempts + 2 + u32::from(ledger_first)
            );
            assert!(expected.work.lease.is_none());
            assert_eq!(
                f.store.agent_log_entry(&f.agent.id, head).await.unwrap(),
                Some(marker)
            );
            assert_eq!(
                f.store
                    .agent_log
                    .count_documents(doc! {"agent_id": f.agent.id.as_str()})
                    .await
                    .unwrap(),
                head
            );
            assert_eq!(f.store.agent_log.count_documents(doc! {
                "agent_id": f.agent.id.as_str(), "record.kind.failure.class": AGENT_RUNTIME_STARTUP_HOLD,
            }).await.unwrap(), 1);
            assert_eq!(f.store.agent_log.count_documents(doc! {
                "agent_id": f.agent.id.as_str(), "record.kind.failure.class": AGENT_RUNTIME_LEDGER_HOLD,
            }).await.unwrap(), u64::from(ledger_first));
            f.cleanup().await;
        }
    }
}
