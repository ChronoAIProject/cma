use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

use super::work_tests::{claim_request, live_request};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn sandbox_maintenance_refuses_command_claims_and_renewal() {
    let (client, name) = database("held_command_claim").await;
    let (store, agent, parent) = fixture(&name, &client).await;
    let store = Arc::new(store);
    let execution = Arc::new(LocalReadiness::default());
    let worker = AgentCommandWorker::new(store.clone(), execution.clone(), "held".into()).unwrap();
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "held"))
            .await
            .unwrap(),
    );
    let replica = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let parents = replica
        .database(&name)
        .collection::<mongodb::bson::Document>("sandboxes");
    let filter = doc! { "_id": parent.id.as_str() };
    let now = layer2_core::now_rfc3339();
    let before = snapshot(&store).await;
    for hold in [
        doc! { "phase": "requested" }.into(),
        mongodb::bson::Bson::Null,
    ] {
        parents
            .update_one(
                filter.clone(),
                doc! { "$set": { "ledger_maintenance": hold } },
            )
            .write_concern(mongodb::options::WriteConcern::majority())
            .await
            .unwrap();
        let held_parent = parents.find_one(filter.clone()).await.unwrap().unwrap();
        assert_eq!(
            store
                .claim_next_command(claim_request(&agent, "held", &now))
                .await,
            Err(CommandLedgerError::StaleClaim)
        );
        assert_eq!(
            worker.run_one(&admitted.receipt.envelope.command_id).await,
            Err(CommandLedgerError::StaleClaim)
        );
        assert_eq!(execution.preparations.load(Ordering::Relaxed), 0);
        assert!(execution.attempts.lock().unwrap().is_empty());
        assert_eq!(snapshot(&store).await, before);
        assert_eq!(
            parents.find_one(filter.clone()).await.unwrap(),
            Some(held_parent)
        );
    }
    parents
        .update_one(
            filter.clone(),
            doc! { "$unset": { "ledger_maintenance": "" } },
        )
        .await
        .unwrap();
    let acquired = observe(&store, &agent, "unheld", &admitted).await.unwrap();
    let lease = acquired.work.lease.unwrap();
    let now = layer2_core::now_rfc3339();
    let expiry =
        layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(45));
    let before = snapshot(&store).await;
    for hold in [
        doc! { "phase": "requested" }.into(),
        mongodb::bson::Bson::Null,
    ] {
        parents
            .update_one(
                filter.clone(),
                doc! { "$set": { "ledger_maintenance": hold } },
            )
            .write_concern(mongodb::options::WriteConcern::majority())
            .await
            .unwrap();
        let held_parent = parents.find_one(filter.clone()).await.unwrap().unwrap();
        assert_eq!(
            store
                .renew_command_claim(&lease.claim, now.clone(), expiry.clone())
                .await,
            Err(CommandLedgerError::StaleClaim)
        );
        assert_eq!(snapshot(&store).await, before);
        assert_eq!(
            parents.find_one(filter.clone()).await.unwrap(),
            Some(held_parent)
        );
    }
    parents
        .update_one(filter, doc! { "$unset": { "ledger_maintenance": "" } })
        .await
        .unwrap();
    store
        .renew_command_claim(&lease.claim, now, expiry.clone())
        .await
        .unwrap();
    let renewed = store
        .get_command(&lease.claim.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(renewed.work.lease.unwrap().expires_at, expiry);
    client.database(&name).drop().await.unwrap();
}

async fn snapshot(store: &MongoSandboxStore) -> Vec<Vec<mongodb::bson::Document>> {
    let mut result = Vec::new();
    for collection in [&store.agent_commands, &store.agent_command_orders] {
        result.push(
            collection
                .find(doc! {})
                .sort(doc! { "_id": 1 })
                .await
                .unwrap()
                .try_collect()
                .await
                .unwrap(),
        );
    }
    result
}

async fn observe(
    store: &MongoSandboxStore,
    agent: &Agent,
    holder: &str,
    command: &AdmittedAgentCommand,
) -> Option<AdmittedAgentCommand> {
    store
        .claim_observed_command(
            claim_request(agent, holder, &layer2_core::now_rfc3339()),
            &command.receipt.envelope.command_id,
            command.revision,
        )
        .await
        .unwrap()
}

// These adapters model different local readiness results only. Attempts are
// counted without deduplication; no runtime or provider authority is simulated.
#[derive(Default)]
struct LocalReadiness {
    unavailable: Option<AgentCommandId>,
    preparations: AtomicUsize,
    attempts: Mutex<Vec<AdmittedAgentCommand>>,
}

#[async_trait::async_trait]
impl AgentCommandExecution for LocalReadiness {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        if self.unavailable.as_ref() == Some(&command.receipt.envelope.command_id) {
            CommandReadiness::Retry
        } else {
            CommandReadiness::Ready
        }
    }
    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        self.preparations.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        self.attempts.lock().unwrap().push(command.clone());
        CommandAttempt::Retry
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn later_local_readiness_cannot_claim_or_mutate_an_unchecked_fifo_command() {
    let (client, name) = database("claim_observed_workers").await;
    let (one, agent, parent) = fixture(&name, &client).await;
    let two = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let first = record(
        one.reserve_command(live_request(&agent, &parent, "first"))
            .await
            .unwrap(),
    );
    let later = record(
        one.reserve_command(live_request(&agent, &parent, "later"))
            .await
            .unwrap(),
    );
    let nonowner = Arc::new(LocalReadiness {
        unavailable: Some(first.receipt.envelope.command_id.clone()),
        ..LocalReadiness::default()
    });
    let before = snapshot(&one).await;
    let worker = AgentCommandWorker::new(two, nonowner.clone(), "nonowner".into()).unwrap();
    for command in [&first, &later] {
        worker
            .run_one(&command.receipt.envelope.command_id)
            .await
            .unwrap();
    }
    assert!(nonowner.attempts.lock().unwrap().is_empty());
    assert_eq!(snapshot(&one).await, before);

    let owner = Arc::new(LocalReadiness::default());
    let one = Arc::new(one);
    AgentCommandWorker::new(one.clone(), owner.clone(), "owner".into())
        .unwrap()
        .run_one(&first.receipt.envelope.command_id)
        .await
        .unwrap();
    {
        let attempts = owner.attempts.lock().unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].receipt, first.receipt);
        assert_eq!(attempts[0].revision, first.revision + 1);
        assert_eq!(attempts[0].work.lease.as_ref().unwrap().claim.fence, 1);
    }
    assert_eq!(
        one.get_command(&later.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(later)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn intervening_replica_claim_and_release_invalidates_the_observed_revision() {
    let (client, name) = database("claim_observed_revision").await;
    let (one, agent, parent) = fixture(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let admitted = record(
        one.reserve_command(live_request(&agent, &parent, "one"))
            .await
            .unwrap(),
    );
    let competing = observe(&two, &agent, "other", &admitted).await.unwrap();
    let old_claim = competing.work.lease.as_ref().unwrap().claim.clone();
    two.release_command_claim(&old_claim, layer2_core::now_rfc3339())
        .await
        .unwrap();
    let current = one
        .get_command(&admitted.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.revision, admitted.revision + 2);
    let before = snapshot(&one).await;
    assert!(observe(&one, &agent, "stale-observer", &admitted)
        .await
        .is_none());
    assert_eq!(snapshot(&two).await, before);

    let fresh = observe(&one, &agent, "fresh-observer", &current)
        .await
        .unwrap();
    assert_eq!(fresh.receipt, admitted.receipt);
    assert_eq!(fresh.key_sha256, admitted.key_sha256);
    assert_eq!(fresh.target_turn, admitted.target_turn);
    assert_eq!(fresh.revision, current.revision + 1);
    assert!(fresh.work.dispatch.is_none());
    let new_claim = &fresh.work.lease.as_ref().unwrap().claim;
    assert_eq!(new_claim.fence, old_claim.fence + 1);
    assert_eq!(
        two.release_command_claim(&old_claim, layer2_core::now_rfc3339())
            .await,
        Err(CommandLedgerError::StaleClaim)
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn competing_fresh_observations_have_one_winner_and_preserve_receipts_and_fences() {
    let (client, name) = database("claim_observed_race").await;
    let (one, agent, parent) = fixture(&name, &client).await;
    let two = MongoSandboxStore::open(&client, &name).await.unwrap();
    let request = live_request(&agent, &parent, "first");
    let first = record(one.reserve_command(request.clone()).await.unwrap());
    let later = record(
        one.reserve_command(live_request(&agent, &parent, "later"))
            .await
            .unwrap(),
    );
    let (left, right) = tokio::join!(
        observe(&one, &agent, "one", &first),
        observe(&two, &agent, "two", &first)
    );
    assert_ne!(left.is_some(), right.is_some());
    let winner = left.or(right).unwrap();
    assert_eq!(winner.receipt, first.receipt);
    assert_eq!(winner.key_sha256, first.key_sha256);
    assert_eq!(winner.revision, first.revision + 1);
    assert!(winner.work.dispatch.is_none());
    let claim = winner.work.lease.as_ref().unwrap().claim.clone();
    assert_eq!(claim.fence, 1);
    let before = snapshot(&one).await;
    assert!(observe(&two, &agent, "skip", &later).await.is_none());
    assert_eq!(snapshot(&two).await, before);
    assert_eq!(record(two.reserve_command(request).await.unwrap()), winner);

    one.reject_command(
        &claim,
        CommandFailureCode::AuthorityRevoked,
        layer2_core::now_rfc3339(),
    )
    .await
    .unwrap();
    let next = observe(&two, &agent, "next", &later).await.unwrap();
    assert_eq!(next.receipt, later.receipt);
    assert_eq!(
        next.work.lease.as_ref().unwrap().claim.fence,
        claim.fence + 1
    );
    let rejected = one
        .get_command(&first.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rejected.receipt, first.receipt);
    assert_eq!(rejected.status, AgentResponseStatus::Failed);
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn observed_selection_preserves_message_holds_and_fifo_controls_for_an_active_turn() {
    let (client, name) = database("claim_observed_hold").await;
    let (store, mut agent, parent) = fixture(&name, &client).await;
    let mut held = record(
        store
            .reserve_command(live_request(&agent, &parent, "held"))
            .await
            .unwrap(),
    );
    // Seed only the stored ledger shape of a held Channel message. This test
    // exercises selection, not Channel admission or authority to release it.
    held.receipt.envelope.actor = AgentActor::Channel {
        bridge_id: "bridge-order".into(),
        grant_id: "grant-order".into(),
        platform: "telegram".into(),
        chat_id: "chat-order".into(),
        sender_id: "sender-order".into(),
    };
    held.receipt.envelope.source.kind = AgentSourceKind::Channel;
    held.status = AgentResponseStatus::WaitingAction;
    store
        .agent_commands
        .replace_one(
            doc! { "_id": held.receipt.envelope.command_id.as_str() },
            crate::store::agent_commands::command_document(&held).unwrap(),
        )
        .await
        .unwrap();
    let later = record(
        store
            .reserve_command(live_request(&agent, &parent, "later"))
            .await
            .unwrap(),
    );
    let before = snapshot(&store).await;
    assert!(observe(&store, &agent, "message", &later).await.is_none());
    assert_eq!(snapshot(&store).await, before);

    agent.capabilities = layer2_core::RuntimeCapabilities::new([
        layer2_core::RuntimeCapability::Conversation,
        layer2_core::RuntimeCapability::Interrupt,
    ]);
    replace_agent_fixture(&store, &agent).await;
    let mutation = begin_stored_conversation_turn(&store, agent).await;
    agent = mutation.agent.clone();
    store.commit_agent_conversation(mutation).await.unwrap();
    let mut controls = Vec::new();
    for key in ["control-one", "control-two"] {
        let mut request = live_request(&agent, &parent, key);
        request.envelope.command = AgentCommand::Interrupt;
        request.envelope.request_fingerprint = request.envelope.command.fingerprint().unwrap();
        request.response_id = None;
        request.queue_capacity = 8;
        controls.push(record(store.reserve_command(request).await.unwrap()));
    }
    let before = snapshot(&store).await;
    assert!(observe(&store, &agent, "later-control", &controls[1])
        .await
        .is_none());
    assert_eq!(snapshot(&store).await, before);
    for control in &controls {
        let selected = observe(&store, &agent, "control", control).await.unwrap();
        assert_eq!(selected.receipt, control.receipt);
        assert_eq!(selected.target_turn, agent.conversation.active_turn);
        assert_eq!(selected.revision, control.revision + 1);
        store
            .reject_command(
                &selected.work.lease.as_ref().unwrap().claim,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    for command in [&held, &later] {
        assert_eq!(
            store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap()
                .as_ref(),
            Some(command)
        );
    }
    client.database(&name).drop().await.unwrap();
}
