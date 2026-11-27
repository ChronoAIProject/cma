use super::*;
use crate::store::agent_ownership::MongoAgentOwnership;
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use layer2_core::sandbox::store::SandboxStoreError;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn retired_cleanup_claim_carries_the_exact_active_proof() {
    let fixture = Fixture::new(true).await;
    fixture.delete("proof-delete").await;
    let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
    let parent = fixture.begin_retirement(now).await;
    fixture.retire(parent, now, true).await;

    let store = fixture
        .h
        .store
        .as_ref()
        .clone()
        .enable_agent_ownership("proof-manager".into())
        .await
        .unwrap()
        .with_prepared_active_proof_for_test(&fixture.agent.id)
        .await;
    let coordinator = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        member("proof-manager"),
    )
    .await
    .unwrap();
    let owner = coordinator
        .acquire(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let proofs = fixture
        .admin
        .collection::<Document>("agent_log_active_proofs_v1");
    let filter = doc! {"_id":fixture.agent.id.as_str()};
    let before = proofs.find_one(filter.clone()).await.unwrap().unwrap();
    let claim = owner
        .run(fixture.claim(&store, "proof-worker", now))
        .await
        .unwrap();
    let carried = proofs.find_one(filter).await.unwrap().unwrap();
    assert_eq!(claim.work.step, AgentWorkStep::EnsureProcessClosed);
    assert_eq!(
        carried.get_i64("agent_revision").unwrap() as u64,
        claim.revision
    );
    assert_eq!(
        before.get_document("head").unwrap(),
        carried.get_document("head").unwrap()
    );
    assert_eq!(before.get("leaves"), carried.get("leaves"));
    assert_ne!(before.get("proof_sha256"), carried.get("proof_sha256"));
    let worker = owned_worker(
        &fixture,
        Arc::new(store),
        "proof-worker",
        coordinator.clone(),
    );
    coordinator.maintain().await.unwrap();
    owner
        .run(worker.execute_claimed_for_test(claim, now))
        .await
        .unwrap()
        .unwrap();
    let now = now.max(ledger_hold_tests::due(&fixture.stored().await));
    fixture.converge(&worker, now).await;
    fixture.admin.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn retired_cleanup_reconciles_a_lost_mongo_commit_and_retains_an_unproved_claim() {
    for (segmented, committed) in [(false, true), (true, true), (false, false), (true, false)] {
        let fixture = Fixture::new(segmented).await;
        fixture.delete("original-delete").await;
        let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
        let parent = fixture.begin_retirement(now).await;
        fixture.retire(parent, now, true).await;
        // Publish WorktreeRemoved under the ambiguous result so reconciliation
        // has to prove the Agent, Worktree, log and Sandbox event together.
        for _ in 0..2 {
            let at = now.max(ledger_hold_tests::due(&fixture.stored().await));
            assert!(fixture
                .h
                .worker
                .run_one(&fixture.agent.id, at)
                .await
                .unwrap());
        }
        let now = now.max(ledger_hold_tests::due(&fixture.stored().await));
        let claim = fixture.claim(&fixture.h.store, "agent-worker", now).await;
        assert_eq!(claim.work.step, AgentWorkStep::RemoveWorktree);
        let commands = fixture.h.backend.requests().await;
        let mut fault = doc! {"failCommands":["commitTransaction"]};
        if committed {
            fault.insert("writeConcernError", doc! {"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"synthetic lost cleanup acknowledgement"});
        } else {
            fault.insert("errorCode", 50_i32);
            fault.insert("errorLabels", vec!["UnknownTransactionCommitResult"]);
        }
        fixture.fail(20, fault).await;
        let result = fixture
            .h
            .worker
            .execute_claimed_for_test(claim.clone(), now)
            .await;
        fixture.clear_fault().await;
        if committed {
            result.unwrap();
            assert_eq!(
                fixture.stored().await.work.step,
                AgentWorkStep::PublishDeleted
            );
            fixture.converge(&fixture.h.worker, now).await;
        } else {
            assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
            assert_eq!(fixture.stored().await, claim);
            assert!(!fixture
                .h
                .worker
                .run_one(&claim.id, now + LEASE - Duration::from_secs(1))
                .await
                .unwrap());
            let session = fixture.commit_session.lock().unwrap().take().unwrap();
            // The server-side failpoint can leave the original transaction
            // open. Abort only this fixture's observed session before drop.
            fixture
                .admin
                .client()
                .database("admin")
                .run_command(doc! {"killSessions":[session]})
                .await
                .unwrap();
        }
        assert_eq!(fixture.h.backend.requests().await, commands);
        fixture.admin.drop().await.unwrap();
    }
}

fn member(name: &str) -> Member {
    Member {
        instance_id: name.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{name}.example:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 16,
    }
}

fn owned_worker(
    fixture: &Fixture,
    store: Arc<MongoSandboxStore>,
    holder: &str,
    coordinator: Arc<OwnershipCoordinator>,
) -> Arc<AgentWorker> {
    let registry = Arc::new(LiveAgentRegistry::default());
    let updates = DurableAgentUpdateRecorder::new(store.clone());
    let operations = AgentOperationService::new(store.clone(), registry.clone());
    AgentWorker::new(
        AgentWorkerDependencies {
            store,
            adapter: fixture.h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry,
            updates,
            operations,
        },
        AgentWorkerConfig::new(holder, DEPLOYMENT_ID).with_ownership(Some(coordinator)),
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable loopback replica set"]
async fn retired_cleanup_preflight_failure_cannot_release_a_successor_epoch() {
    let fixture = Fixture::new(false).await;
    fixture.delete("original-delete").await;
    let now = SystemTime::now().max(ledger_hold_tests::due(&fixture.stored().await));
    let parent = fixture.begin_retirement(now).await;
    fixture.retire(parent, now, true).await;
    let owners = Arc::new(
        MongoAgentOwnership::open(fixture.admin.client(), fixture.admin.name())
            .await
            .unwrap(),
    );
    let old = OwnershipCoordinator::register(owners.clone(), member("old-owner"))
        .await
        .unwrap();
    let old_authority = old.acquire(&fixture.agent.id).await.unwrap().unwrap();
    let old_store = Arc::new(
        fixture
            .h
            .store
            .as_ref()
            .clone()
            .enable_agent_ownership("old-owner".into())
            .await
            .unwrap(),
    );
    let claim = old_authority
        .run(fixture.claim(&old_store, "old-worker", now))
        .await
        .unwrap();
    let old_worker = owned_worker(&fixture, old_store, "old-worker", old);
    owners.heartbeat("old-owner", true).await.unwrap();
    owners.release(&old_authority.token).await.unwrap();
    let next = OwnershipCoordinator::register(owners, member("new-owner"))
        .await
        .unwrap();
    let authority = next.acquire(&claim.id).await.unwrap().unwrap();
    assert!(authority.token.epoch > old_authority.token.epoch);
    // Before the new holder claims work, the child's revision and lease still
    // match. Only the ownership fence can reject the old retry publication.
    fixture
        .fail(1, doc! {"failCommands":["find"],"errorCode":2_i32})
        .await;
    let result = old_authority
        .run(old_worker.execute_claimed_for_test(claim.clone(), now))
        .await;
    fixture.clear_fault().await;
    assert!(matches!(result, Ok(Err(SandboxStoreError::Conflict))));
    assert_eq!(fixture.stored().await, claim);
    let new_store = Arc::new(
        fixture
            .h
            .store
            .as_ref()
            .clone()
            .enable_agent_ownership("new-owner".into())
            .await
            .unwrap(),
    );
    let successor = authority
        .run(fixture.claim(&new_store, "new-worker", now))
        .await
        .unwrap();
    assert!(
        successor.work.lease.as_ref().unwrap().fence > claim.work.lease.as_ref().unwrap().fence
    );

    fixture
        .fail(1, doc! {"failCommands":["find"],"errorCode":2_i32})
        .await;
    let result = old_authority
        .run(old_worker.execute_claimed_for_test(claim, now))
        .await;
    fixture.clear_fault().await;
    assert!(matches!(result, Ok(Err(SandboxStoreError::Conflict))));
    assert_eq!(fixture.stored().await, successor);
    let worker = owned_worker(&fixture, new_store, "new-worker", next);
    authority
        .run(worker.execute_claimed_for_test(successor, now))
        .await
        .unwrap()
        .unwrap();
    fixture.converge(&worker, now).await;
    fixture.admin.drop().await.unwrap();
}
