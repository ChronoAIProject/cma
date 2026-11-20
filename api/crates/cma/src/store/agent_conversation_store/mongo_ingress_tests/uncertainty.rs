use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::Duration;

use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope,
};
use layer2_core::sandbox::store::{AgentConversationStore, AgentLogStore};
use layer3_adapter::RuntimeOutputReceipt;
use mongodb::{
    bson::doc, event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client,
};
use tokio::sync::{mpsc, Notify};

use super::fixture::Fixture;
use crate::store::sandbox_mongo::MongoSandboxStore;

const WORK_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct DriverGate {
    armed: AtomicBool,
    commits: AtomicUsize,
    commit_reply: BlockingGate,
    read_started: BlockingGate,
    read_finished: Notify,
    reading: AtomicBool,
    read_failed: AtomicBool,
    read_scope: Mutex<Option<usize>>,
}

#[derive(Default)]
struct BlockingGate {
    reached: Notify,
    released: Mutex<bool>,
    release: Condvar,
    timed_out: AtomicBool,
}

impl BlockingGate {
    fn hold(&self) {
        self.reached.notify_one();
        let (released, timeout) = self
            .release
            .wait_timeout_while(
                self.released.lock().unwrap(),
                Duration::from_secs(20),
                |released| !*released,
            )
            .unwrap();
        self.timed_out.store(timeout.timed_out(), Ordering::SeqCst);
        drop(released);
    }

    fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.release.notify_all();
    }
}

impl DriverGate {
    fn handler(self: &Arc<Self>) -> EventHandler<CommandEvent> {
        let gate = self.clone();
        // The pinned driver spawns async callbacks separately. A synchronous
        // callback holds the actual operation and observes its task-local scope.
        EventHandler::callback(move |event| match event {
            CommandEvent::Succeeded(event)
                if gate.armed.load(Ordering::SeqCst)
                    && event.command_name == "commitTransaction" =>
            {
                // The fifth write-concern error exhausts the existing
                // unknown-commit retry loop. Hold the actual driver reply.
                if gate.commits.fetch_add(1, Ordering::SeqCst) == 4 {
                    gate.commit_reply.hold();
                }
            }
            CommandEvent::Started(event)
                if gate.armed.load(Ordering::SeqCst)
                    && gate.commits.load(Ordering::SeqCst) == 5
                    && event.command_name == "find"
                    && event
                        .command
                        .get_document("readConcern")
                        .is_ok_and(|concern| {
                            matches!(concern.get_str("level"), Ok("snapshot"))
                        }) =>
            {
                *gate.read_scope.lock().unwrap() =
                    IngressWorkScope::current().map(|scope| scope.requested_bytes());
                gate.reading.store(true, Ordering::SeqCst);
                gate.read_started.hold();
            }
            CommandEvent::Succeeded(event)
                if event.command_name == "find" && gate.reading.swap(false, Ordering::SeqCst) =>
            {
                gate.read_finished.notify_one();
            }
            CommandEvent::Failed(event)
                if event.command_name == "find" && gate.reading.swap(false, Ordering::SeqCst) =>
            {
                gate.read_failed.store(true, Ordering::SeqCst);
                gate.read_finished.notify_one();
            }
            _ => {}
        })
    }
}

async fn reached(event: &Notify) -> Result<(), &'static str> {
    tokio::time::timeout(Duration::from_secs(10), event.notified())
        .await
        .map_err(|_| "driver did not reach the expected uncertainty boundary")
}

fn scope() -> (IngressBudget, IngressWorkScope) {
    let root = IngressBudget::new(128 * 1024 * 1024);
    let plan = |bytes| AllocationPlan::new("real reconciliation fixture", &[bytes]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), plan(1024 * 1024), plan(WORK_BYTES)).unwrap(),
    )
    .unwrap();
    let scope = IngressWorkScope::new(
        policy
            .reserve(IngressClass::Transformation, plan(WORK_BYTES))
            .unwrap(),
    );
    (root, scope)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn committed_reply_and_cancelled_readback_keep_actual_checkpoint_proof_charged() {
    cancelled_readback(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn unavailable_cancelled_readback_keeps_actual_checkpoint_proof_charged() {
    cancelled_readback(true).await;
}

async fn cancelled_readback(unavailable_read: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let app_name = format!("cma-ingress-uncertainty-{}", uuid::Uuid::new_v4().simple());
    let gate = Arc::new(DriverGate::default());
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    options.command_event_handler = Some(gate.handler());
    let mut fixture =
        Fixture::with_client(Client::with_options(options).unwrap(), "uncertainty").await;
    let previous = fixture.retain_prefix(1024 * 1024 - 1).await;
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let observer = MongoSandboxStore::open(&admin, &fixture.database)
        .await
        .unwrap();
    let original_logs = observer.agent_log(&fixture.agent.id, 0, 100).await.unwrap();
    let mut next = previous.clone();
    next.update_id = uuid::Uuid::new_v4().to_string();
    next.update_fingerprint = "c".repeat(64);
    let source = next.output.as_mut().unwrap();
    source.cursor += 1;
    source.payload_sha256 = "d".repeat(64);
    let source = source.clone();
    let (send_ack, mut acknowledged) = mpsc::channel(1);
    let receipt = RuntimeOutputReceipt::new(source.clone(), send_ack);
    let retained_input = Arc::new((fixture.agent.clone(), previous.clone(), next.clone()));
    let task_store = fixture.store.clone();
    let task_receipt = receipt.clone();
    let (root, scope) = scope();
    admin.database("admin").run_command(doc! {
        "configureFailPoint": "failCommand", "mode": {"times": 5_i32},
        "data": {"failCommands": ["commitTransaction"], "appName": &app_name,
            "writeConcernError": {"code": 50_i32, "codeName": "MaxTimeMSExpired", "errmsg": "synthetic uncertain ingress commit"}},
    }).await.unwrap();
    gate.armed.store(true, Ordering::SeqCst);
    let caller = tokio::spawn(async move {
        scope
            .run(async move {
                // These real near-limit proof copies are created after admission
                // and move into the production commit/reconciliation future.
                let result = task_store
                    .commit_agent_message_update(
                        retained_input.0.clone(),
                        Some(retained_input.1.clone()),
                        retained_input.2.clone(),
                        None,
                    )
                    .await;
                if result.is_ok() {
                    task_receipt.acknowledge().await.unwrap();
                }
                result
            })
            .await
    });

    let observed = async {
        reached(&gate.commit_reply.reached).await?;
        // The commit may have applied before the majority snapshot advances.
        // Keep its reply withheld until an independent client sees the write.
        let committed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let committed = observer
                    .agent_message_checkpoint(&fixture.agent.id)
                    .await
                    .unwrap();
                if committed.as_ref() == Some(&next) {
                    break committed;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .map_err(|_| "committed checkpoint did not become independently visible")?;
        let early_ack = acknowledged.try_recv().ok();
        if unavailable_read {
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint": "failCommand", "mode": "alwaysOn",
                    "data": {"failCommands": ["find"], "appName": &app_name,
                        "errorCode": 2_i32},
                })
                .await
                .unwrap();
        }
        gate.commit_reply.release();
        reached(&gate.read_started.reached).await?;
        caller.abort();
        // Cancellation must be observed before inspecting the remaining owner.
        while !caller.is_finished() {
            tokio::task::yield_now().await;
        }
        Ok::<_, &'static str>((
            committed,
            early_ack,
            root.available_bytes(),
            *gate.read_scope.lock().unwrap(),
        ))
    }
    .await;

    // Always release real driver work and disable the scoped failpoint before
    // assertions, including when this regression is run against the old hook.
    gate.armed.store(false, Ordering::SeqCst);
    gate.commit_reply.release();
    gate.read_started.release();
    caller.abort();
    let cancelled = caller.await;
    let read_completed = if observed.is_ok() {
        reached(&gate.read_finished).await
    } else {
        Ok(())
    };
    admin
        .database("admin")
        .run_command(doc! {"configureFailPoint": "failCommand", "mode": "off"})
        .await
        .unwrap();
    let settled = tokio::time::timeout(Duration::from_secs(5), async {
        while root.available_bytes() != root.capacity_bytes() - root.metadata_bytes() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let before_retry_ack = acknowledged.try_recv().ok();
    let retry = observer
        .commit_agent_message_update(fixture.agent.clone(), Some(previous), next.clone(), None)
        .await;
    if retry.is_ok() {
        receipt.acknowledge().await.unwrap();
    }
    let exact_ack = acknowledged.try_recv().ok();
    let duplicate_ack = acknowledged.try_recv().ok();
    let final_checkpoint = observer
        .agent_message_checkpoint(&fixture.agent.id)
        .await
        .unwrap();
    let final_logs = observer.agent_log(&fixture.agent.id, 0, 100).await.unwrap();
    fixture.finish().await;
    drop(observer);
    tokio::time::timeout(Duration::from_secs(5), admin.shutdown())
        .await
        .expect("observer Mongo resources did not finish cleanup");

    assert!(!gate.commit_reply.timed_out.load(Ordering::SeqCst));
    assert!(!gate.read_started.timed_out.load(Ordering::SeqCst));
    let (committed, early_ack, held, child_scope) = observed.unwrap();
    assert!(cancelled.unwrap_err().is_cancelled());
    read_completed.unwrap();
    assert_eq!(gate.read_failed.load(Ordering::SeqCst), unavailable_read);
    settled.unwrap();
    assert!(
        committed.as_ref() == Some(&next),
        "commit is durable before its reply is released"
    );
    assert!(early_ack.is_none() && before_retry_ack.is_none());
    assert_eq!(retry.unwrap(), next.outcome);
    assert_eq!(exact_ack, Some(source));
    assert!(duplicate_ack.is_none());
    assert!(final_checkpoint.as_ref() == Some(&next));
    assert_eq!(final_logs, original_logs);
    assert_eq!(
        held,
        root.capacity_bytes() - root.metadata_bytes() - WORK_BYTES,
        "cancelled caller released a live Mongo proof"
    );
    assert_eq!(child_scope, Some(WORK_BYTES));
}
