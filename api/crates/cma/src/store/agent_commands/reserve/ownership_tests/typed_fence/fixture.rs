//! App-scoped faults reach the public admission CAS in its real transaction.
//! Each retry retains the original request revision and resource lineage.
use crate::store::{agent_ownership::MongoAgentOwnership, sandbox_mongo::MongoSandboxStore};
use layer2_core::agent_ownership::{Authority, Member, OwnershipCoordinator, PROTOCOL_VERSION};
use layer2_core::AgentId;
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Clone)]
struct Fault {
    remaining: usize,
    labels: Vec<String>,
    delay: bool,
    nth_fence: usize,
}
#[derive(Default)]
struct Trace {
    fault: Option<Fault>,
    commands: Vec<Document>,
    hits: usize,
    fences_in_transaction: usize,
    injected: Vec<i32>,
    failures: Vec<(i32, i32, bool, bool)>,
}
pub(in crate::store) struct Fixture {
    pub store: MongoSandboxStore,
    pub owners: Arc<MongoAgentOwnership>,
    admin: Client,
    name: String,
    trace: Arc<Mutex<Trace>>,
    notified: Arc<Notify>,
    resume: Arc<Notify>,
}
pub(in crate::store) fn member(id: &str) -> Member {
    Member {
        instance_id: id.into(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: 256,
    }
}
impl Fixture {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
            .expect("disposable failpoint Mongo required");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let name = format!("m1_leaf_fence_{suffix}");
        let app = format!("m1-leaf-fence-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let trace = Arc::new(Mutex::new(Trace::default()));
        let notified = Arc::new(Notify::new());
        let resume = Arc::new(Notify::new());
        let continuation = resume.clone();
        let (observed, control, notification) = (trace.clone(), admin.clone(), notified.clone());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let (observed, control, app, notification) = (
                observed.clone(),
                control.clone(),
                app.clone(),
                notification.clone(),
            );
            let continuation = continuation.clone();
            // Async command callbacks are detached by the driver. A synchronous
            // callback is required to arm the fault or finish takeover before
            // this exact wire command can leave the client.
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                if let CommandEvent::Failed(event) = &event {
                    let mut trace = observed.lock().unwrap();
                    if trace.injected.contains(&event.request_id) {
                        let code = match event.failure.kind.as_ref() {
                            mongodb::error::ErrorKind::Command(error) => error.code,
                            _ => -1,
                        };
                        trace.failures.push((
                            event.request_id,
                            code,
                            event.failure.contains_label("TransientTransactionError"),
                            event
                                .failure
                                .contains_label("UnknownTransactionCommitResult"),
                        ));
                    }
                    return;
                }
                let CommandEvent::Started(event) = event else {
                    return;
                };
                {
                    let mut trace = observed.lock().unwrap();
                    if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                        trace.fences_in_transaction = 0;
                    }
                    if is_admission_fence(&event.command) {
                        trace.fences_in_transaction += 1;
                    }
                }
                let wait_for_takeover = matches!(event.command.get_bool("startTransaction"), Ok(true)) && observed
                    .lock()
                    .unwrap()
                    .fault
                    .as_ref()
                    .is_some_and(|f| f.delay && f.remaining == 0);
                if wait_for_takeover {
                    observed.lock().unwrap().fault.as_mut().unwrap().delay = false;
                    notification.notify_one();
                    tokio::time::timeout(
                        std::time::Duration::from_secs(10),
                        continuation.notified(),
                    )
                    .await
                    .unwrap();
                }
                let fault = {
                    let mut trace = observed.lock().unwrap();
                    trace.commands.push(event.command.clone());
                    if !is_admission_fence(&event.command) {
                        return;
                    }
                    let member_index = trace.fences_in_transaction;
                    let Some(fault) = trace.fault.as_mut() else {
                        return;
                    };
                    if fault.remaining == 0 || member_index != fault.nth_fence {
                        return;
                    }
                    fault.remaining -= 1;
                    let fault = fault.clone();
                    trace.hits += 1;
                    trace.injected.push(event.request_id);
                    fault
                };
                let data = doc! {"appName":app,"failCommands":["update"],"errorCode":112_i32,"errorLabels":fault.labels};
                control
                    .database("admin")
                    .run_command(
                        doc! {"configureFailPoint":"failCommand","mode":{"times":1},"data":data},
                    )
                    .await
                    .unwrap();
            })
            })
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let owners = Arc::new(MongoAgentOwnership::open(&admin, &name).await.unwrap());
        Self {
            store,
            owners,
            admin,
            name,
            trace,
            notified,
            resume,
        }
    }
    pub async fn own(&mut self, id: &AgentId) -> Arc<Authority> {
        self.store = self
            .store
            .clone()
            .enable_agent_ownership("leaf-owner".into())
            .await
            .unwrap();
        OwnershipCoordinator::register(self.owners.clone(), member("leaf-owner"))
            .await
            .unwrap()
            .acquire(id)
            .await
            .unwrap()
            .unwrap()
    }
    pub fn arm(&self, failures: usize, labels: &[&str], delay: bool, nth_fence: usize) {
        let mut t = self.trace.lock().unwrap();
        t.commands.clear();
        t.hits = 0;
        t.fences_in_transaction = 0;
        t.injected.clear();
        t.failures.clear();
        t.fault = Some(Fault {
            remaining: failures,
            labels: labels.iter().map(|s| s.to_string()).collect(),
            delay,
            nth_fence,
        });
    }
    pub async fn takeover(&self, old: &Authority) {
        tokio::time::timeout(std::time::Duration::from_secs(5), self.notified.notified())
            .await
            .unwrap();
        // Public admission uses the original resource revision, not a runtime
        // ownership token. Advance that revision before its fresh snapshot.
        let changed = self
            .admin
            .database(&self.name)
            .collection::<Document>("agents")
            .update_one(
                doc! {"_id": old.token.agent_id.as_str()},
                doc! {"$inc": {"revision": 1_i64, "record.revision": 1_i64}},
            )
            .await
            .unwrap();
        assert_eq!(changed.matched_count, 1);
        assert!(old.is_current());
        self.resume.notify_one();
    }
    pub async fn clear(&self) -> (usize, Vec<Document>, Vec<(i32, i32, bool, bool)>) {
        self.trace.lock().unwrap().fault = None;
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        let t = self.trace.lock().unwrap();
        (t.hits, t.commands.clone(), t.failures.clone())
    }
    pub async fn cleanup(&self) {
        self.admin.database(&self.name).drop().await.unwrap();
    }
}
pub(in crate::store) fn assert_attempts(commands: &[Document], expected: usize) {
    let starts: Vec<_> = commands
        .iter()
        .filter(|c| matches!(c.get_bool("startTransaction"), Ok(true)))
        .collect();
    assert_eq!(
        starts.len(),
        expected,
        "must retry only known-aborted snapshots within the budget"
    );
    for (index, c) in starts.iter().enumerate() {
        for prior in &starts[..index] {
            assert!(
                c.get_document("lsid").unwrap() != prior.get_document("lsid").unwrap()
                    || c.get_i64("txnNumber").unwrap() != prior.get_i64("txnNumber").unwrap(),
                "must use a fresh transaction"
            );
        }
    }
}

fn is_admission_fence(command: &Document) -> bool {
    command.get_str("update").ok() == Some("agents")
        && command.get_array("updates").is_ok_and(|updates| {
            updates.iter().any(|update| {
                update
                    .as_document()
                    .and_then(|row| row.get_document("u").ok())
                    .and_then(|change| change.get_document("$inc").ok())
                    .is_some_and(|fields| fields.contains_key("agent_command_admission_fence"))
            })
        })
}
