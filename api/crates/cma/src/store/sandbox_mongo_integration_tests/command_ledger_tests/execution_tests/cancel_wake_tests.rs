use super::*;
use layer2_core::agent_ownership::{
    execution_owner, Member, OwnershipCoordinator, OwnershipStore, OwnershipToken, PROTOCOL_VERSION,
};
use tokio::sync::{mpsc, oneshot, watch, Notify};

async fn replica(
    store: &MongoSandboxStore,
    instance: &str,
) -> (Arc<MongoSandboxStore>, Arc<OwnershipCoordinator>) {
    let store = Arc::new(
        store
            .clone()
            .enable_agent_ownership(instance.into())
            .await
            .unwrap(),
    );
    let coordinator = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        Member {
            instance_id: instance.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://fixture.invalid".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 1,
        },
    )
    .await
    .unwrap();
    (store, coordinator)
}

#[derive(Default)]
struct ScopedControl(Mutex<Vec<(AgentCommandId, OwnershipToken)>>);

#[async_trait::async_trait]
impl AgentCommandExecution for ScopedControl {
    async fn check_queued(&self, _: &AdmittedAgentCommand) -> CommandReadiness {
        CommandReadiness::Ready
    }

    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        panic!("an active Cancel can be claimed directly")
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        assert_eq!(command.receipt.envelope.command, AgentCommand::Interrupt);
        self.0.lock().unwrap().push((
            command.receipt.envelope.command_id.clone(),
            execution_owner().expect("Cancel runs under the acquired owner epoch"),
        ));
        CommandAttempt::Reject(CommandFailureCode::AgentUnavailable)
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn periodic_tick_recovers_a_committed_cancel_without_a_wake_after_handoff() {
    let (client, name) = database("cancel_wake_handoff").await;
    let (store, mut agent, parent, _, _, _) = composed(&client, &name).await;
    agent.status = AgentStatus::Working;
    agent.capabilities = RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Interrupt,
    ]);
    agent.conversation.active_turn = Some(1);
    store
        .agents
        .replace_one(
            doc! { "_id": agent.id.as_str() },
            crate::store::agents::agent_document(&agent).unwrap(),
        )
        .await
        .unwrap();

    let (old_store, first) = replica(&store, "first-cancel-manager").await;
    let first_authority = first.acquire(&agent.id).await.unwrap().unwrap();
    let (store, second) = replica(&store, "successor-cancel-manager").await;
    let execution = Arc::new(ScopedControl::default());
    let old_worker = AgentCommandWorker::new(
        old_store.clone(),
        execution.clone(),
        "old-cancel-worker".into(),
    )
    .unwrap()
    .with_ownership(Some(first.clone()));
    let (old_observed, mut old_scans) = mpsc::channel(1);
    let mut old_scheduler = crate::commands::CommandScheduler::new(old_worker)
        .observe_discovery(old_observed, None)
        .driven();
    old_scheduler.poll().await;
    assert!(old_scans.try_recv().unwrap().is_empty());

    let worker = AgentCommandWorker::new(store.clone(), execution.clone(), "cancel-worker".into())
        .unwrap()
        .with_ownership(Some(second.clone()));
    let (observed, mut scans) = mpsc::channel(8);
    let (release, first_scan_release) = oneshot::channel();
    let (shutdown, signal) = watch::channel(false);
    let scheduler = crate::commands::CommandScheduler::new(worker)
        .with_cancel_wake(Some(Arc::new(Notify::new())))
        .observe_discovery(observed, Some(first_scan_release))
        .spawn(signal);
    assert!(tokio::time::timeout(Duration::from_secs(5), scans.recv())
        .await
        .unwrap()
        .unwrap()
        .is_empty());

    // The old scheduler was driven through one pass and remains stopped at
    // that boundary. The successor's empty scan holds its discovery slot.
    let mut reservation = live_request(&agent, &parent, "missed-cancel-wake");
    reservation.envelope.command = AgentCommand::Interrupt;
    reservation.envelope.request_fingerprint = AgentCommand::Interrupt.fingerprint().unwrap();
    reservation.response_id = None;
    let cancel = record(old_store.reserve_command(reservation).await.unwrap());
    let id = cancel.receipt.envelope.command_id;
    assert_eq!(
        old_store
            .agent_ownership
            .as_ref()
            .unwrap()
            .owner(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .token,
        first_authority.token,
    );
    let pending = store.get_command(&id).await.unwrap().unwrap();
    assert_eq!(pending.status, AgentResponseStatus::Queued);
    assert!(pending.work.lease.is_none());
    assert!(execution.0.lock().unwrap().is_empty());

    first.begin_drain().await;
    first
        .handoff(&first_authority, async { Ok(()) })
        .await
        .unwrap();
    let owner = second.acquire(&agent.id).await.unwrap().unwrap();
    assert!(owner.token.epoch > first_authority.token.epoch);
    release.send(()).unwrap();

    // The advisory notification is unset. Only a periodic tick can discover
    // the pending receipt after the successor completes its initial pass.
    let discovery = tokio::time::timeout(Duration::from_secs(5), scans.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(discovery.as_slice(), std::slice::from_ref(&id));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store
                .get_command(&id)
                .await
                .unwrap()
                .unwrap()
                .status
                .is_terminal()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(tokio::time::timeout(Duration::from_secs(5), scans.recv())
        .await
        .unwrap()
        .unwrap()
        .is_empty());
    assert_eq!(
        execution.0.lock().unwrap().as_slice(),
        &[(id.clone(), owner.token.clone())]
    );
    eprintln!("committed_before_handoff=true wake_sent=false old_epoch={} successor_epoch={} successor_executions=1",
        first_authority.token.epoch, owner.token.epoch);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
    client.database(&name).drop().await.unwrap();
}
