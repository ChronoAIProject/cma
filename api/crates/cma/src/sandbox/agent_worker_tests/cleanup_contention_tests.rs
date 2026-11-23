//! Definite Mongo publication aborts must not strand a cleanup claim for 18 minutes.
use super::*;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::store::AgentStopRequest;
use mongodb::{bson::doc, options::ClientOptions, Client};

mod retirement;

const STEPS: [AgentWorkStep; 7] = [
    AgentWorkStep::FenceInteraction,
    AgentWorkStep::CloseProcess,
    AgentWorkStep::PublishStopped,
    AgentWorkStep::EnsureProcessClosed,
    AgentWorkStep::RemoveHome,
    AgentWorkStep::RemoveWorktree,
    AgentWorkStep::PublishDeleted,
];

async fn prepare<S: FixtureStore>(h: &Harness<S>, wanted: AgentWorkStep) -> Agent {
    let key = format!("cleanup-conflict-{wanted:?}-{}", uuid::Uuid::new_v4());
    let admitted = h.admit(&key, vec![]).await;
    if wanted == AgentWorkStep::EnsureProcessClosed {
        // A lost OpenAgent response and definitive reconnect refusal leave a
        // failed child with possible runtime effects but no CloseProcess receipt.
        // This exercises the external ensure-close operation, not its skip path.
        for _ in 0..2 {
            h.run(&admitted.id, SystemTime::now()).await;
        }
        h.backend.lose_next_response(WireOperation::OpenAgent);
        h.run(&admitted.id, SystemTime::now()).await;
        h.backend.fail_next(
            WireOperation::OpenAgent,
            "request_rejected",
            "synthetic rejection",
        );
        let current = h.stored(&admitted.id).await;
        assert!(h
            .worker
            .run_one(
                &admitted.id,
                SystemTime::now().max(ledger_hold_tests::due(&current))
            )
            .await
            .unwrap());
        let failed = h.stored(&admitted.id).await;
        assert_eq!(failed.status, AgentStatus::Failed);
        h.store
            .schedule_agent_delete(AgentDeleteRequest {
                target: AgentControlTarget::from(&failed),
                requested_at: rfc3339_at(SystemTime::now().max(ledger_hold_tests::due(&failed))),
            })
            .await
            .unwrap();
        return h.stored(&admitted.id).await;
    }
    for _ in 0..20 {
        let current = h.stored(&admitted.id).await;
        if current.work.step == wanted {
            return current;
        }
        let now = SystemTime::now().max(ledger_hold_tests::due(&current));
        match (current.status, current.work.step) {
            (AgentStatus::Ready, AgentWorkStep::None) => {
                h.store
                    .schedule_agent_stop(AgentStopRequest {
                        target: AgentControlTarget::from(&current),
                        reason: AgentStopReason::Owner,
                        requested_at: rfc3339_at(now),
                    })
                    .await
                    .unwrap();
            }
            (AgentStatus::Stopped, AgentWorkStep::None) => {
                h.store
                    .schedule_agent_delete(AgentDeleteRequest {
                        target: AgentControlTarget::from(&current),
                        requested_at: rfc3339_at(now),
                    })
                    .await
                    .unwrap();
            }
            _ => assert!(h.worker.run_one(&current.id, now).await.unwrap()),
        }
    }
    panic!("fixture never reached {wanted:?}");
}

async fn claimed_cleanup() -> (Harness, Agent, SystemTime) {
    let h = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let original = prepare(&h, AgentWorkStep::RemoveWorktree).await;
    let now = SystemTime::now().max(ledger_hold_tests::due(&original));
    let claimed = h
        .store
        .claim_agent_work(
            &original.id,
            "agent-worker",
            rfc3339_at(now),
            rfc3339_at(now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    (h, claimed, now)
}

#[tokio::test]
async fn cleanup_conflict_keeps_concurrent_metadata_and_retries_original_step() {
    let (h, claimed, now) = claimed_cleanup().await;
    let mut updated = claimed.clone();
    updated.title = Some("Owner title during cleanup".into());
    updated.revision += 1;
    h.store
        .save_agent(claimed.revision, updated.clone())
        .await
        .unwrap();
    h.worker
        .finish_cleanup_attempt(
            &claimed,
            Err(layer2_core::sandbox::store::SandboxStoreError::Conflict),
            now,
        )
        .await
        .unwrap();
    let retry = h.stored(&claimed.id).await;
    ledger_hold_tests::preserved(&updated, &retry);
    assert_eq!(retry.title, updated.title);
    assert_eq!(retry.work.attempts, 1);
    assert!(ledger_hold_tests::due(&retry) < now + Duration::from_secs(5));
    assert_eq!(finish(&h, &claimed.id).await.status, AgentStatus::Deleted);
}

#[tokio::test]
async fn cleanup_unknown_commits_and_replaced_or_expired_claims_are_untouched() {
    use layer2_core::sandbox::store::SandboxStoreError;
    for replace in [false, true] {
        let (h, claimed, now) = claimed_cleanup().await;
        for result in [
            Ok(()),
            Err(SandboxStoreError::Unavailable("unknown commit".into())),
            Err(SandboxStoreError::AgentLogCapacityExceeded),
        ] {
            assert_eq!(
                h.worker
                    .finish_cleanup_attempt(&claimed, result.clone(), now)
                    .await,
                result
            );
            assert_eq!(h.stored(&claimed.id).await, claimed);
        }
        let later = now + Duration::from_secs(31);
        let expected = if replace {
            h.store
                .claim_agent_work(
                    &claimed.id,
                    "replacement",
                    rfc3339_at(later),
                    rfc3339_at(later + Duration::from_secs(30)),
                )
                .await
                .unwrap()
                .unwrap()
        } else {
            claimed.clone()
        };
        assert_eq!(
            h.worker
                .finish_cleanup_attempt(&claimed, Err(SandboxStoreError::Conflict), later)
                .await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(h.stored(&claimed.id).await, expected);
    }
}

#[tokio::test]
async fn cleanup_claim_refresh_accepts_only_display_and_log_metadata() {
    use super::super::agent_worker::same_cleanup_claim;
    let (_h, claimed, now) = claimed_cleanup().await;
    let mut metadata = claimed.clone();
    metadata.revision += 1;
    metadata.conversation.last_seq += 1;
    metadata.title = Some("Renamed".into());
    metadata.updated_at = rfc3339_at(now + Duration::from_secs(1));
    assert!(same_cleanup_claim(&claimed, &metadata, "agent-worker", now));
    for changed in [
        "fence", "holder", "step", "runtime", "stop", "profile", "parent", "input", "attempts",
    ] {
        let mut candidate = metadata.clone();
        match changed {
            "fence" => candidate.work.lease.as_mut().unwrap().fence += 1,
            "holder" => candidate.work.lease.as_mut().unwrap().holder = "replacement".into(),
            "step" => candidate.work.step = AgentWorkStep::PublishDeleted,
            "runtime" => candidate.conversation.runtime_generation = Some(99),
            "stop" => {
                candidate.requested_stop_reason = if candidate.requested_stop_reason.is_some() {
                    None
                } else {
                    Some(AgentStopReason::Owner)
                }
            }
            "profile" => candidate.profile.content.system_prompt = "changed".into(),
            "parent" => candidate.sandbox_id = SandboxId::generate(),
            "input" => {
                candidate.conversation.first_message =
                    Some(FirstMessageRecord::new("late", "new input", rfc3339_at(now)).unwrap())
            }
            "attempts" => candidate.work.attempts += 1,
            _ => unreachable!(),
        }
        assert!(
            !same_cleanup_claim(&claimed, &candidate, "agent-worker", now),
            "{changed}"
        );
    }
    let mut startup = claimed.clone();
    startup.work.step = AgentWorkStep::AddWorktree;
    assert!(!same_cleanup_claim(&startup, &startup, "agent-worker", now));
}

async fn finish<S: FixtureStore>(h: &Harness<S>, id: &AgentId) -> Agent {
    for _ in 0..12 {
        let current = h.stored(id).await;
        if current.status == AgentStatus::Deleted {
            return current;
        }
        let now = SystemTime::now().max(ledger_hold_tests::due(&current));
        if current.status == AgentStatus::Stopped && current.work.step == AgentWorkStep::None {
            h.store
                .schedule_agent_delete(AgentDeleteRequest {
                    target: AgentControlTarget::from(&current),
                    requested_at: rfc3339_at(now),
                })
                .await
                .unwrap();
        } else {
            assert!(h.worker.run_one(id, now).await.unwrap());
        }
    }
    panic!("child cleanup did not converge");
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn cleanup_publication_conflicts_release_only_the_original_claim_and_finish_same_child() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app = format!("cma-cleanup-conflict-{suffix}");
    let database = format!("cma_cleanup_conflict_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(MongoSandboxStore::open(&client, &database).await.unwrap());
    let mut h = Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
    h.now = SystemTime::now();
    let sandbox_before = store
        .get_sandbox(&h.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    for step in STEPS {
        // One abort is absorbed by Mongo's transaction loop. Five exhaust its
        // complete known-abort budget, with no remaining injected failures.
        for failures in [1_i32, 5] {
            let original = prepare(&h, step).await;
            let now = SystemTime::now().max(ledger_hold_tests::due(&original));
            let claimed = store
                .claim_agent_work(
                    &original.id,
                    "agent-worker",
                    rfc3339_at(now),
                    rfc3339_at(now + Duration::from_secs(18 * 60)),
                )
                .await
                .unwrap()
                .unwrap();
            let before_requests = h.backend.requests().await.len();
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint": "failCommand", "mode": { "times": failures },
                    "data": { "failCommands": ["update"], "appName": &app,
                        "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"] },
                })
                .await
                .unwrap();
            let result = h
                .worker
                .execute_claimed_for_test(claimed.clone(), now)
                .await;
            admin
                .database("admin")
                .run_command(doc! { "configureFailPoint": "failCommand", "mode": "off" })
                .await
                .expect("disable the app-scoped failpoint before assertions");
            result.unwrap_or_else(|error| panic!("{step:?}/{failures}: {error}"));
            let after = h.stored(&original.id).await;
            assert!(after.work.lease.is_none(), "{step:?}/{failures}");
            if failures == 5 {
                assert_eq!(after.work.step, step);
                assert_eq!(after.work.attempts, 1);
                assert_eq!(after.status, claimed.status);
                assert_eq!(after.work.receipts, claimed.work.receipts);
                assert_eq!(after.work.cleanup_debt, claimed.work.cleanup_debt);
                assert!(ledger_hold_tests::due(&after) < now + Duration::from_secs(5));
            }
            let completed = finish(&h, &original.id).await;
            assert_eq!(completed.work.step, AgentWorkStep::None);
            assert!(completed.work.lease.is_none());
            assert_eq!(completed.id, original.id);
            assert_eq!(completed.worktree_id, original.worktree_id);
            assert_eq!(completed.workspace_id, original.workspace_id);
            assert_eq!(completed.sandbox_id, original.sandbox_id);
            let operation = match step {
                AgentWorkStep::CloseProcess | AgentWorkStep::EnsureProcessClosed => {
                    Some(WireOperation::CloseAgent)
                }
                AgentWorkStep::RemoveHome => Some(WireOperation::RemoveAgentHome),
                AgentWorkStep::RemoveWorktree => Some(WireOperation::RemoveWorktree),
                _ => None,
            };
            if let Some(operation) = operation {
                let identities: Vec<_> = h.backend.requests().await[before_requests..]
                    .iter()
                    .filter(|(_, request)| request.operation() == operation)
                    .map(|(_, request)| ledger_hold_tests::request_identity(request))
                    .collect();
                assert_eq!(identities.len(), if failures == 5 { 2 } else { 1 });
                for pair in identities.windows(2) {
                    assert_eq!(pair[0].1, pair[1].1, "cleanup operation identity changed");
                    assert_eq!(pair[0].2, pair[1].2, "cleanup request hash changed");
                    assert!(
                        pair[0].0 < pair[1].0,
                        "successor must acquire fresh authority"
                    );
                }
            }
        }
    }
    let sandbox_after = store
        .get_sandbox(&h.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox_after.status, SandboxStatus::Active);
    assert_eq!(sandbox_after.runtime, sandbox_before.runtime);
    assert_eq!(
        store
            .get_workspace(&h.workspace.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        WorkspaceStatus::Ready
    );
    client.database(&database).drop().await.unwrap();
}
