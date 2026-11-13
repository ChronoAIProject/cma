//! Compose actual scheduling, command authority, Mongo claims and dispatch.
use super::*;
use crate::worker_supervision::{Activity, Workers};
use tokio::{sync::Notify, time::Instant};

struct GatedExecution {
    inner: Arc<crate::commands::ResourceCommandExecution>,
    claims: Mutex<Vec<CommandClaim>>,
    claimed: Notify,
    release: Notify,
}

#[async_trait::async_trait]
impl AgentCommandExecution for GatedExecution {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        self.inner.check_queued(command).await
    }
    async fn prepare_queued(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        self.inner.prepare_queued(command).await
    }
    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        let first = {
            let mut claims = self.claims.lock().unwrap();
            claims.push(command.work.lease.as_ref().unwrap().claim.clone());
            claims.len() == 1
        };
        if first {
            self.claimed.notify_one();
            self.release.notified().await;
        }
        self.inner.execute(command).await
    }
}

async fn state(activity: &Activity, expected: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while activity.snapshot().state != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn dispatched(store: &MongoSandboxStore, id: &AgentCommandId) -> AdmittedAgentCommand {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let command = store.get_command(id).await.unwrap().unwrap();
            assert!(command.work.failure.is_none(), "{command:?}");
            if command.work.dispatch.is_some() {
                break command;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

async fn finish(store: &Arc<MongoSandboxStore>, id: &AgentCommandId) -> AdmittedAgentCommand {
    let command = store.get_command(id).await.unwrap().unwrap();
    let agent = store
        .get_agent(&command.receipt.envelope.agent_id)
        .await
        .unwrap()
        .unwrap();
    DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            agent.conversation.runtime_generation.unwrap(),
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            // Only the actual scheduler drives reconcile_command after the runtime update.
            let current = store.get_command(id).await.unwrap().unwrap();
            if current.status == AgentResponseStatus::Completed {
                break current;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

async fn recover(stall: bool) {
    let (client, name) = database(if stall {
        "scheduler_stall"
    } else {
        "scheduler_panic"
    })
    .await;
    let (store, agent, parent, execution, _, sends) = composed(&client, &name).await;
    let reservation = live_request(&agent, &parent, "retained-work");
    let original = record(store.reserve_command(reservation.clone()).await.unwrap());
    let id = &original.receipt.envelope.command_id;
    let execution = Arc::new(GatedExecution {
        inner: execution,
        claims: Mutex::default(),
        claimed: Notify::new(),
        release: Notify::new(),
    });
    let worker = AgentCommandWorker::new(
        store.clone(),
        execution.clone(),
        "supervised-replica".into(),
    )
    .unwrap();
    let activity = Workers::new().get("agent_commands");
    let (stop, signal) = tokio::sync::watch::channel(false);
    let scheduler = crate::commands::CommandScheduler::new(worker.clone())
        .spawn_supervised(signal, activity.clone());
    tokio::time::timeout(Duration::from_secs(15), execution.claimed.notified())
        .await
        .unwrap();
    let claimed = store.get_command(id).await.unwrap().unwrap();
    let claim = claimed.work.lease.as_ref().unwrap().claim.clone();
    assert_eq!(claimed.receipt, original.receipt);
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    let fault_at = Instant::now();
    if stall {
        crate::worker_supervision::stall_next_poll(&activity);
    } else {
        crate::worker_supervision::panic_next_poll(&activity);
    }
    state(&activity, "backoff").await;
    let detection = fault_at.elapsed();
    assert!(detection < Duration::from_secs(10));
    assert!(!activity.snapshot().available());
    state(&activity, "running").await;
    let scheduling = fault_at.elapsed();
    // Recovery retained the original child and its durable claim, with no second execution.
    let retained = store.get_command(id).await.unwrap().unwrap();
    assert_eq!(retained.work.lease.as_ref().unwrap().claim, claim);
    assert_eq!(
        execution.claims.lock().unwrap().as_slice(),
        std::slice::from_ref(&claim)
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
    execution.release.notify_one();
    let sent = dispatched(&store, id).await;
    assert_eq!(sent.work.dispatch.as_ref().unwrap().claim, claim);
    let done = finish(&store, id).await;
    let durable = fault_at.elapsed();
    assert!(durable < Duration::from_secs(30));
    assert_eq!(done.receipt, original.receipt);
    assert_eq!(done.work.dispatch, sent.work.dispatch);
    assert!(done.work.lease.is_none());
    assert_eq!(sends.load(Ordering::SeqCst), 1);

    // A fresh admitted command must also progress under the recovered scheduler.
    let ready = store.get_agent(&agent.id).await.unwrap().unwrap();
    let next = record(
        store
            .reserve_command(live_request(&ready, &parent, "subsequent-work"))
            .await
            .unwrap(),
    );
    let next_id = &next.receipt.envelope.command_id;
    dispatched(&store, next_id).await;
    assert_eq!(finish(&store, next_id).await.receipt, next.receipt);
    let next_durable = fault_at.elapsed();
    assert!(next_durable < Duration::from_secs(30));
    stop.send(true).unwrap();
    scheduler.await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        record(reopened.reserve_command(reservation).await.unwrap()),
        done
    );
    worker.run_one(id).await.unwrap();
    worker.run_one(next_id).await.unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    assert_eq!(execution.claims.lock().unwrap().len(), 2);
    assert_eq!(
        reopened
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    let logs = reopened.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|log| matches!(log.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        2
    );
    println!("scheduler durable recovery: stall={stall} detection_ms={} scheduling_ms={} completed_ms={} subsequent_completed_ms={} effects=2 claims=2 receipts=2",
        detection.as_millis(), scheduling.as_millis(), durable.as_millis(), next_durable.as_millis());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn command_scheduler_panic_retains_durable_claim_and_completes_once() {
    recover(false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn command_scheduler_stall_retains_durable_claim_and_completes_once() {
    recover(true).await;
}
