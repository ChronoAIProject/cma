use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::Duration;

use layer2_core::sandbox::ingress_ownership::{
    AllocationPlan, IngressBudget, IngressClass, IngressOwnershipPolicy, IngressWorkScope,
    OwnedWork, SharedOwned,
};
use layer2_core::sandbox::store::{
    AgentConversationStore, AgentMessageSnapshot, SandboxStoreError,
};
use layer2_core::AgentId;
use mongodb::{event::command::CommandEvent, event::EventHandler, options::ClientOptions, Client};
use tokio::sync::Notify;

use super::fixture::Fixture;
use crate::store::sandbox_mongo::MongoSandboxStore;

#[derive(Default)]
struct ReadGate {
    armed: AtomicBool,
    queries: AtomicUsize,
    available_at_query: AtomicUsize,
    started: Notify,
    released: Mutex<bool>,
    release: Condvar,
    timed_out: AtomicBool,
}

impl ReadGate {
    fn handler(self: &Arc<Self>, root: IngressBudget) -> EventHandler<CommandEvent> {
        let gate = self.clone();
        // Synchronous callbacks hold the actual operation, including its read
        // allowance. An async callback would observe an independently spawned task.
        EventHandler::callback(move |event| {
            let CommandEvent::Started(event) = event else {
                return;
            };
            if event.command_name != "find" || !gate.armed.load(Ordering::SeqCst) {
                return;
            }
            if gate.queries.fetch_add(1, Ordering::SeqCst) != 0 {
                return;
            }
            gate.available_at_query
                .store(root.available_bytes(), Ordering::SeqCst);
            gate.started.notify_one();
            let (released, timeout) = gate
                .release
                .wait_timeout_while(
                    gate.released.lock().unwrap(),
                    Duration::from_secs(20),
                    |released| !*released,
                )
                .unwrap();
            gate.timed_out.store(timeout.timed_out(), Ordering::SeqCst);
            drop(released);
        })
    }

    fn finish(&self) {
        *self.released.lock().unwrap() = true;
        self.release.notify_all();
    }
}

enum Snapshot {
    Admitted(SharedOwned<AgentMessageSnapshot>),
    Legacy(Box<AgentMessageSnapshot>),
}

impl std::ops::Deref for Snapshot {
    type Target = AgentMessageSnapshot;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Admitted(snapshot) => snapshot,
            Self::Legacy(snapshot) => snapshot,
        }
    }
}

async fn read(
    store: &MongoSandboxStore,
    id: &AgentId,
    policy: &IngressOwnershipPolicy,
    admitted: bool,
) -> Result<Option<Snapshot>, OwnedWork<SandboxStoreError>> {
    if admitted {
        store
            .admitted_agent_message_snapshot(id, policy)
            .await
            .map(|snapshot| snapshot.map(Snapshot::Admitted))
    } else {
        match store.agent_message_snapshot(id).await {
            Ok(snapshot) => Ok(snapshot.map(|snapshot| Snapshot::Legacy(Box::new(snapshot)))),
            Err(error) => Err(IngressWorkScope::inherit_owned(async { error }).await),
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn blocked_snapshot_refuses_competing_read_before_query_and_keeps_control_capacity() {
    blocked_read(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
#[should_panic(expected = "snapshot query reached Mongo without its read allowance")]
async fn legacy_read_control_fails_the_same_pre_query_admission_assertion() {
    blocked_read(false).await;
}

async fn blocked_read(admitted: bool) {
    let (root, policy) = super::policy();
    let gate = Arc::new(ReadGate::default());
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    options.command_event_handler = Some(gate.handler(root.clone()));
    let mut fixture =
        Fixture::with_client(Client::with_options(options).unwrap(), "pressure").await;
    let checkpoint = fixture.retain_prefix(1024 * 1024 - 1).await;
    let task_store = fixture.store.clone();
    let task_id = fixture.agent.id.clone();
    let task_policy = policy.clone();
    gate.armed.store(true, Ordering::SeqCst);
    let first =
        tokio::spawn(async move { read(&task_store, &task_id, &task_policy, admitted).await });

    let observed = tokio::time::timeout(Duration::from_secs(5), async {
        gate.started.notified().await;
        let held = root.available_bytes();
        let second = read(&fixture.store, &fixture.agent.id, &policy, admitted).await;
        let refused =
            matches!(&second, Err(error) if matches!(&**error, SandboxStoreError::Unavailable(_)));
        drop(second);
        let control = policy
            .admit(
                IngressClass::Control,
                AllocationPlan::checked("snapshot pressure control", &[4096]).unwrap(),
            )
            .map_err(|_| "control capacity was consumed by the blocked read")?
            .initialize(|| vec![13_u8; 1024]);
        let control_correct = control.iter().all(|byte| *byte == 13);
        drop(control);
        Ok::<_, &'static str>((
            held,
            refused,
            gate.queries.load(Ordering::SeqCst),
            root.available_bytes(),
            control_correct,
            first.is_finished(),
        ))
    })
    .await;

    // Release the real driver operation and clean its database/client before
    // asserting, including when the same assertion runs against the legacy path.
    gate.finish();
    let first = tokio::time::timeout(Duration::from_secs(5), first).await;
    gate.armed.store(false, Ordering::SeqCst);
    let remaining_with_result = root.available_bytes();
    let transformation_after_read = policy
        .budget(IngressClass::Transformation)
        .available_bytes();
    let matches = match &first {
        Ok(Ok(Ok(Some(snapshot)))) => snapshot.checkpoint.as_ref() == Some(&checkpoint),
        _ => false,
    };
    drop(first);
    let remaining_after_drop = root.available_bytes();
    fixture.finish().await;

    assert!(!gate.timed_out.load(Ordering::SeqCst));
    let (held, refused, queries, after_attempt, control_correct, finished) =
        observed.unwrap().unwrap();
    assert!(!finished, "the first snapshot must still be blocked");
    assert!(
        matches,
        "the retained prefix and exact receipt must survive the read"
    );
    assert!(control_correct);
    assert_eq!(remaining_after_drop, root.capacity_bytes());
    eprintln!(
        "SNAPSHOT_READ_PROOF admitted={admitted} queries_while_blocked={queries} read_reserved_bytes={} result_reserved_bytes={} control_completed={control_correct}",
        root.capacity_bytes() - gate.available_at_query.load(Ordering::SeqCst),
        root.capacity_bytes() - remaining_with_result,
    );
    assert!(
        gate.available_at_query.load(Ordering::SeqCst) < root.capacity_bytes(),
        "snapshot query reached Mongo without its read allowance"
    );
    assert!(
        refused,
        "competing read must refuse without waiting for the first read"
    );
    assert_eq!(queries, 1, "refused read must not issue a Mongo query");
    assert_eq!(
        after_attempt, held,
        "refusal must return any partial admission"
    );
    assert_eq!(
        transformation_after_read,
        policy.bounds().transformation_bytes()
    );
    assert!(remaining_with_result < root.capacity_bytes());
    assert!(
        remaining_with_result > held,
        "returned data must release read work capacity"
    );
}
