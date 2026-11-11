use super::*;
use crate::sandbox::agent_worker_tests::ledger_hold_tests as hold;
use layer2_core::sandbox::store::{
    plan_agent_runtime_ledger_hold_retry, AgentMessageCheckpoint, SandboxStoreError,
};
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateOutcome};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn ledger_hold_mongo_all_paths_preserve_pending_work_at_full_history_and_takeover() {
    for (step, op, next) in hold::CASES {
        let (client, name, h) = fixture().await;
        let initial = hold::prepare(&h, step).await;
        fill_history(&client, &name, &h, &initial.id).await;
        let parent = h.store.get_sandbox(&initial.sandbox_id).await.unwrap();
        let mut previous = h.stored(&initial.id).await;
        let mut at = hold::due(&previous).max(SystemTime::now());
        let start = h.backend.requests().await.len();
        for (attempt, delay) in [30, 60, 120, 240, 300].into_iter().enumerate() {
            let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
            let worker = hold::worker(&h, store, &format!("replacement-{attempt}"));
            hold::refuse(&h, op);
            assert!(worker.run_one(&initial.id, at).await.unwrap(), "{step:?}");
            let held = h.stored(&initial.id).await;
            if attempt > 0
                || !matches!(
                    step,
                    AgentWorkStep::AddWorktree
                        | AgentWorkStep::MaterializeHome
                        | AgentWorkStep::OpenConversation
                )
            {
                hold::preserved(&previous, &held);
            }
            assert_eq!(held.work.step, step);
            assert_eq!(held.status, initial.status);
            assert_eq!(held.work.attempts, attempt as u32 + 1);
            assert_eq!(
                held.conversation.last_seq, 10_001,
                "hold diagnostics must stay bounded"
            );
            assert!(held.work.lease.is_none());
            assert_eq!(
                layer2_core::seconds_from_rfc3339(&held.work.ready_at).unwrap()
                    - layer2_core::seconds_from_rfc3339(&held.updated_at).unwrap(),
                delay
            );
            assert!(!worker
                .run_one(&initial.id, hold::due(&held) - Duration::from_secs(1))
                .await
                .unwrap());
            assert_eq!(
                h.store.get_sandbox(&initial.sandbox_id).await.unwrap(),
                parent
            );
            at = hold::due(&held);
            previous = held;
        }
        let requests = h.backend.requests().await;
        let identities: Vec<_> = requests[start..]
            .iter()
            .filter(|(_, request)| request.operation() == op)
            .map(|(_, request)| hold::request_identity(request))
            .collect();
        assert_eq!(identities.len(), 5);
        for pair in identities.windows(2) {
            assert!(pair[0].0 < pair[1].0);
            assert_eq!((&pair[0].1, &pair[0].2), (&pair[1].1, &pair[1].2));
        }
        // Existing cleanup reserves still allow actual successful effects.
        // Full normal history remains a separate limit for startup/reconnect.
        if matches!(
            step,
            AgentWorkStep::CloseProcess
                | AgentWorkStep::EnsureProcessClosed
                | AgentWorkStep::RemoveHome
                | AgentWorkStep::RemoveWorktree
        ) {
            assert!(hold::worker(&h, h.store.clone(), "success")
                .run_one(&initial.id, at)
                .await
                .unwrap());
            let advanced = h.stored(&initial.id).await;
            assert_eq!(advanced.work.step, next);
            assert_eq!(advanced.work.attempts, 0);
            assert!(advanced.work.lease.is_none());
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn ledger_hold_mongo_keeps_active_turn_partial_answer_and_output_receipt() {
    let (client, name, h) = fixture().await;
    let initial = hold::prepare(&h, AgentWorkStep::PublishConnection).await;
    let current = fill_history(&client, &name, &h, &initial.id).await;
    let mut assembly = serde_json::to_value(AgentMessageAssembly::default()).unwrap();
    assembly["message_id"] = serde_json::json!("synthetic-message");
    assembly["content"] = serde_json::json!("synthetic partial output");
    let checkpoint = AgentMessageCheckpoint {
        version: 1,
        update_id: uuid::Uuid::new_v4().to_string(),
        update_fingerprint: "a".repeat(64),
        output: Some(layer3_adapter::RuntimeOutputSource {
            stream_id: uuid::Uuid::new_v4().to_string(),
            cursor: 17,
            native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
            payload_sha256: "b".repeat(64),
        }),
        native_conversation_id: current.conversation.conversation_id.clone().unwrap(),
        runtime_generation: current.conversation.runtime_generation.unwrap(),
        active_turn: current.conversation.active_turn,
        after_seq: current.conversation.last_seq,
        assembly: serde_json::from_value(assembly).unwrap(),
        outcome: AgentUpdateOutcome {
            appended: vec![],
            turn_closed: false,
        },
    };
    h.store
        .commit_agent_message_update(current.clone(), None, checkpoint.clone(), None)
        .await
        .unwrap();
    let mut at = SystemTime::now();
    for attempt in 0..3 {
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        hold::refuse(&h, WireOperation::OpenAgent);
        assert!(
            hold::worker(&h, store.clone(), &format!("reconnect-{attempt}"))
                .run_one(&initial.id, at)
                .await
                .unwrap()
        );
        let held = h.stored(&initial.id).await;
        hold::preserved(&current, &held);
        assert!(held.conversation.active_turn.is_some());
        let actual = store
            .agent_message_checkpoint(&initial.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(actual.output, checkpoint.output);
        assert_eq!(actual.assembly, checkpoint.assembly);
        assert_eq!(actual.active_turn, checkpoint.active_turn);
        assert_eq!(
            actual.native_conversation_id,
            checkpoint.native_conversation_id
        );
        assert_eq!(actual.after_seq, held.conversation.last_seq);
        at = hold::due(&held);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn ledger_hold_mongo_refuses_unproved_empty_retry_changed_state_and_stale_lease() {
    let (client, name, h) = fixture().await;
    let initial = hold::prepare(&h, AgentWorkStep::PublishConnection).await;
    let at = SystemTime::now();
    let claimed = h
        .store
        .claim_agent_work(
            &initial.id,
            "holder",
            rfc3339_at(at),
            rfc3339_at(at + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    let plan = plan_agent_runtime_ledger_hold_retry(
        &claimed,
        None,
        "holder",
        claimed.work.lease.as_ref().unwrap().fence,
        &rfc3339_at(at),
        &rfc3339_at(at + Duration::from_secs(30)),
    )
    .unwrap();
    let mut empty = plan.clone();
    empty.agent_logs.clear();
    empty.agent.conversation.last_seq = claimed.conversation.last_seq;
    assert_eq!(
        h.store.commit_agent_work(empty).await,
        Err(SandboxStoreError::Conflict)
    );
    let mut cleared = plan.clone();
    cleared.agent.conversation.active_turn = None;
    assert_eq!(
        h.store.commit_agent_work(cleared).await,
        Err(SandboxStoreError::Conflict)
    );
    let mut receipts = plan.clone();
    receipts.agent.work.receipts.clear();
    assert_eq!(
        h.store.commit_agent_work(receipts).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(h.stored(&initial.id).await, claimed);
    let later = at + Duration::from_secs(31);
    let replacement = h
        .store
        .claim_agent_work(
            &initial.id,
            "replacement",
            rfc3339_at(later),
            rfc3339_at(later + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        h.store.commit_agent_work(plan).await,
        Err(SandboxStoreError::Conflict)
    );
    assert_eq!(h.stored(&initial.id).await, replacement);
    client.database(&name).drop().await.unwrap();
}
