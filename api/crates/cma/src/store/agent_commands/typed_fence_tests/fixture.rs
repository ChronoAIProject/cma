//! App-scoped faults are armed at the exact durable member write, not at an
//! unrelated earlier Agent CAS. Every test owns its database and drops it.
use crate::store::{agent_ownership::MongoAgentOwnership, sandbox_mongo::MongoSandboxStore};
use layer2_core::agent_ownership::{
    Authority, Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
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
    collection: &'static str,
    remaining: usize,
    labels: Vec<String>,
    delay: bool,
}
#[derive(Default)]
struct Trace {
    fault: Option<Fault>,
    commands: Vec<Document>,
    hits: usize,
    failed: usize,
    injected: std::collections::BTreeMap<i32, Vec<String>>,
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
        let name = format!("m2_typed_fence_{suffix}");
        let app = format!("m2-typed-fence-{suffix}");
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
            if let CommandEvent::Failed(event) = &event {
                let mut trace = observed.lock().unwrap();
                if let Some(labels) = trace.injected.remove(&event.request_id) {
                    assert!(
                        matches!(event.failure.kind.as_ref(), mongodb::error::ErrorKind::Command(error) if error.code == 112)
                    );
                    for label in labels {
                        assert!(event.failure.contains_label(label));
                    }
                    trace.failed += 1;
                }
                return;
            }
            // AsyncCallback is spawned by the driver and does not fence the wire write.
            // A synchronous callback on a multithread runtime makes ordering explicit.
            tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async move {
                let CommandEvent::Started(event) = event else {
                    return;
                };
                let wait_for_takeover = {
                    let mut trace = observed.lock().unwrap();
                    if let Some(fault) = trace.fault.as_mut() {
                        let wait = matches!(event.command.get_bool("startTransaction"), Ok(true))
                            && fault.delay && fault.remaining == 0;
                        if wait { fault.delay = false; }
                        wait
                    } else { false }
                };
                if wait_for_takeover {
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
                    let Some(fault) = trace.fault.as_mut() else {
                        return;
                    };
                    if event.command.get_str("update").ok() != Some(fault.collection) {
                        return;
                    }
                    if fault.remaining == 0 {
                        return;
                    }
                    fault.remaining -= 1;
                    let fault = fault.clone();
                    trace.hits += 1;
                    trace.injected.insert(event.request_id, fault.labels.clone());
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
            .enable_agent_ownership("m2-original".into())
            .await
            .unwrap();
        OwnershipCoordinator::register(self.owners.clone(), member("m2-original"))
            .await
            .unwrap()
            .acquire(id)
            .await
            .unwrap()
            .unwrap()
    }
    pub fn arm(&self, failures: usize, labels: &[&str], delay: bool) {
        let mut t = self.trace.lock().unwrap();
        t.commands.clear();
        t.hits = 0;
        t.failed = 0;
        t.injected.clear();
        t.fault = Some(Fault {
            collection: "cma_agent_manager_members",
            remaining: failures,
            labels: labels.iter().map(|s| s.to_string()).collect(),
            delay,
        });
    }
    pub fn arm_commands(&self, failures: usize, labels: &[&str]) {
        self.arm(failures, labels, false);
        self.trace
            .lock()
            .unwrap()
            .fault
            .as_mut()
            .unwrap()
            .collection = "agent_commands";
    }
    pub async fn takeover(&self, old: &Authority) {
        tokio::time::timeout(std::time::Duration::from_secs(5), self.notified.notified())
            .await
            .unwrap();
        // The failed transaction has aborted. Hold the next snapshot
        // before its first command until independent durable takeover is complete.
        self.owners.heartbeat("m2-original", true).await.unwrap();
        self.owners.release(&old.token).await.unwrap();
        self.owners.register(member("m2-successor")).await.unwrap();
        let next = self
            .owners
            .acquire(&old.token.agent_id, "m2-successor")
            .await
            .unwrap()
            .unwrap();
        assert!(next.token.epoch > old.token.epoch);
        assert!(old.is_current());
        self.resume.notify_one();
    }
    pub async fn clear(&self) -> (usize, Vec<Document>) {
        self.trace.lock().unwrap().fault = None;
        self.admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        let t = self.trace.lock().unwrap();
        assert!(
            t.injected.is_empty(),
            "every injected member write must join a failure"
        );
        assert_eq!(
            t.failed, t.hits,
            "every member fault must return code 112 and its labels"
        );
        (t.hits, t.commands.clone())
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
