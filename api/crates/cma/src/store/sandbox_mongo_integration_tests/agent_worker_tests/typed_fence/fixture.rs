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
pub(super) struct Fixture {
    pub store: MongoSandboxStore,
    pub owners: Arc<MongoAgentOwnership>,
    admin: Client,
    name: String,
    commands: Arc<Mutex<Vec<Document>>>,
    app: String,
    prior_hits: Mutex<i64>,
}
pub(super) fn member(id: &str) -> Member {
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
        let name = format!("m3_typed_fence_{suffix}");
        let app = format!("m3-typed-fence-{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let seen = commands.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        // The synchronous callback records only. Async command callbacks are
        // spawned by the driver and cannot order a fault before a command.
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                seen.lock().unwrap().push(event.command);
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let owners = Arc::new(MongoAgentOwnership::open(&admin, &name).await.unwrap());
        Self {
            store,
            owners,
            admin,
            name,
            commands,
            app,
            prior_hits: Mutex::new(0),
        }
    }
    pub async fn own(&mut self, id: &AgentId) -> Arc<Authority> {
        self.store = self
            .store
            .clone()
            .enable_agent_ownership("m3-original".into())
            .await
            .unwrap();
        OwnershipCoordinator::register(self.owners.clone(), member("m3-original"))
            .await
            .unwrap()
            .acquire(id)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn arm(&self, failures: usize, labels: &[&str], delay: bool) {
        self.commands.lock().unwrap().clear();
        let mut data = doc! {
            "appName": &self.app, "failCommands": ["update"],
            "errorCode": 112_i32, "errorLabels": labels.to_vec(),
        };
        if delay {
            data.insert("blockConnection", true);
            data.insert("blockTimeMS", 2000_i32);
        }
        let result = self
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand",
                "mode": {"times": i32::try_from(failures).unwrap()},
                "data": data,
            })
            .await
            .unwrap();
        *self.prior_hits.lock().unwrap() = count(&result);
    }
    pub async fn takeover(&self, old: &Authority) {
        let prior = *self.prior_hits.lock().unwrap();
        self.admin
            .database("admin")
            .run_command(doc! {
                "waitForFailPoint": "failCommand", "timesEntered": prior + 1,
                "maxTimeMS": 5000_i32,
            })
            .await
            .expect("the real server must reach the blocked ownership update");
        // failCommand blocks before executing the first member update. Complete
        // takeover during that known-aborted attempt; the next snapshot must
        // reject the old epoch even though its process-local token is current.
        self.owners.heartbeat("m3-original", true).await.unwrap();
        self.owners.release(&old.token).await.unwrap();
        self.owners.register(member("m3-successor")).await.unwrap();
        let next = self
            .owners
            .acquire(&old.token.agent_id, "m3-successor")
            .await
            .unwrap()
            .unwrap();
        assert!(next.token.epoch > old.token.epoch);
        assert!(old.is_current());
    }
    pub async fn clear(&self) -> (usize, Vec<Document>) {
        let result = self
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .unwrap();
        let hits = count(&result) - *self.prior_hits.lock().unwrap();
        (
            usize::try_from(hits).unwrap(),
            self.commands.lock().unwrap().clone(),
        )
    }
    pub async fn cleanup(&self) {
        self.admin.database(&self.name).drop().await.unwrap();
    }
}
pub(super) fn assert_attempts(commands: &[Document], expected: usize) {
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

fn count(document: &Document) -> i64 {
    document
        .get_i64("count")
        .or_else(|_| document.get_i32("count").map(i64::from))
        .expect("server failpoint count must be an integer")
}
