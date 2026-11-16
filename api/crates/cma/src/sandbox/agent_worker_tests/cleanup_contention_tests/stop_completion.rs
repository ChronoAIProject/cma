//! Stop must recover a failed close publication without waiting for claim expiry.
use super::*;
use layer2_core::sandbox::{AgentTitle, AgentTitleStore};
use layer2_core::{Authorization, Caller, Capability};
use layer3_adapter::{PromptResponse, TurnStopReason};

async fn interrupt_before_stop(h: &Harness<MongoSandboxStore>, ready: Agent) -> Agent {
    let caller = Caller::new(
        ready.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(h.store.clone(), h.registry.clone());
    let sent = operations
        .send_message(&caller, &ready.id, "Continue until interrupted.".into())
        .await
        .unwrap();
    assert!(matches!(
        sent,
        layer2_core::sandbox::AgentMessageOutcome::Dispatched(_)
    ));
    assert_eq!(h.stored(&ready.id).await.status, AgentStatus::Working);
    operations.interrupt(&caller, &ready.id).await.unwrap();
    h.backend
        .send_agent_update(
            &h.runtime,
            &AgentSlot::new(ready.id.to_string()),
            ConversationUpdate::TurnEnded(PromptResponse::new(TurnStopReason::Cancelled)),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let current = h.stored(&ready.id).await;
            if current.status == AgentStatus::Ready && current.conversation.active_turn.is_none() {
                let logs = h.store.agent_log(&ready.id, 0, 100).await.unwrap();
                assert_eq!(
                    logs.iter()
                        .filter(|entry| matches!(entry.kind, AgentLogKind::TurnInterrupted { .. }))
                        .count(),
                    1
                );
                return current;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("runtime cancellation must be recorded before lifecycle Stop")
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn stop_completes_in_seconds_after_close_publication_conflict() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("cma-stop-seconds-{suffix}");
    let database = format!("cma_stop_seconds_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let mut h = Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
    h.now = SystemTime::now();
    h.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(store.clone(), h.registry.clone()),
        },
        AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID)
            .with_work_retry_policy(true)
            .with_cleanup_continuation(true),
    );

    // The control case distinguishes normal Stop from the failed-claim path.
    for (failures, metadata_changed) in [(0_i32, false), (5, false), (0, true)] {
        let agent = h
            .admit(
                &format!("stop-seconds-{failures}-{metadata_changed}"),
                vec![],
            )
            .await;
        for _ in 0..5 {
            h.run(&agent.id, SystemTime::now()).await;
        }
        let ready = h.stored(&agent.id).await;
        assert_eq!(ready.status, AgentStatus::Ready);
        assert_eq!(ready.work.step, AgentWorkStep::None);
        let ready = interrupt_before_stop(&h, ready).await;
        let sandbox = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
        let before_requests = h.backend.requests().await.len();
        let started = std::time::Instant::now();
        let accepted_at = SystemTime::now();
        store
            .schedule_agent_stop(AgentStopRequest {
                target: AgentControlTarget::from(&ready),
                reason: AgentStopReason::Owner,
                requested_at: rfc3339_at(accepted_at),
            })
            .await
            .unwrap();
        h.run(&agent.id, SystemTime::now()).await;
        let close = h.stored(&agent.id).await;
        assert_eq!(close.work.step, AgentWorkStep::CloseProcess);
        let claimed = store
            .claim_agent_work(
                &agent.id,
                "agent-worker",
                rfc3339_at(SystemTime::now()),
                rfc3339_at(SystemTime::now() + Duration::from_secs(18 * 60)),
            )
            .await
            .unwrap()
            .unwrap();
        if metadata_changed {
            store
                .set_user_agent_title(
                    &agent.id,
                    &AgentTitle::new("Renamed during cleanup").unwrap(),
                    rfc3339_at(SystemTime::now()),
                )
                .await
                .unwrap();
            let renamed = h.stored(&agent.id).await;
            assert!(renamed.revision > claimed.revision);
            assert_eq!(renamed.work, claimed.work);
            assert_eq!(renamed.title.as_deref(), Some("Renamed during cleanup"));
        }
        if failures != 0 {
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint": "failCommand", "mode": { "times": failures },
                    "data": { "failCommands": ["update"], "appName": &app,
                        "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"] },
                })
                .await
                .unwrap();
        }
        let result = h
            .worker
            .execute_claimed_for_test(claimed.clone(), SystemTime::now())
            .await;
        admin
            .database("admin")
            .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
            .await
            .expect("disable the app-scoped failpoint before assertions");
        result.expect("a definite close publication abort schedules the same work");
        let after = h.stored(&agent.id).await;
        assert!(after.work.lease.is_none());
        assert_eq!(
            after.status,
            AgentStatus::Ready,
            "only PublishStopped is terminal"
        );
        if failures != 0 || metadata_changed {
            assert_eq!(after.work.step, AgentWorkStep::CloseProcess);
            assert_eq!(after.work.attempts, 1);
            assert_eq!(after.work.receipts, claimed.work.receipts);
            assert_eq!(after.work.cleanup_debt, claimed.work.cleanup_debt);
        }

        // Use wall time and ordinary worker claims: advancing to ready_at would
        // hide the original 1,080-second lease regression.
        let stopped = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let current = h.stored(&agent.id).await;
                if current.status == AgentStatus::Stopped {
                    break current;
                }
                h.worker
                    .run_scheduled(&agent.id, SystemTime::now())
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("Stop must complete in seconds, without expiry of its 18-minute claim");
        assert!(started.elapsed() < Duration::from_secs(15));
        assert_eq!(stopped.work.step, AgentWorkStep::None);
        assert!(stopped.work.lease.is_none());
        assert_eq!(stopped.stop_reason, Some(AgentStopReason::Owner));
        let requests = h.backend.requests().await;
        let closes: Vec<_> = requests[before_requests..]
            .iter()
            .map(|(_, request)| {
                assert_eq!(request.operation(), WireOperation::CloseAgent);
                ledger_hold_tests::request_identity(request)
            })
            .collect();
        assert_eq!(
            closes.len(),
            if failures == 0 && !metadata_changed {
                1
            } else {
                2
            }
        );
        for pair in closes.windows(2) {
            assert!(pair[0].0 < pair[1].0);
            assert_eq!(pair[0].1, pair[1].1, "retry preserves operation identity");
            assert_eq!(pair[0].2, pair[1].2, "retry preserves request digest");
        }
        let snapshot = h.backend.snapshot(&h.runtime).await.unwrap();
        assert!(snapshot
            .homes
            .contains_key(&AgentSlot::new(agent.id.to_string())));
        let parent = store.get_sandbox(&ready.sandbox_id).await.unwrap().unwrap();
        assert_eq!(parent.status, SandboxStatus::Active);
        assert_eq!(parent.runtime, sandbox.runtime);
        if metadata_changed {
            assert_eq!(stopped.title.as_deref(), Some("Renamed during cleanup"));
        }
        assert_eq!(
            store
                .get_workspace(&ready.workspace_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorkspaceStatus::Ready
        );
        assert_eq!(
            store
                .get_worktree(&ready.worktree_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            WorktreeStatus::Ready
        );
        println!(
            "stop_completion aborts={failures} metadata_changed={metadata_changed} elapsed_ms={} terminal=stopped",
            started.elapsed().as_millis()
        );
    }
    client.database(&database).drop().await.unwrap();
}
