use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::fixture;
use layer2_core::agent_ownership::{Member, OwnershipCoordinator, DEFAULT_MAX_OWNED_AGENTS};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use std::collections::HashSet;
use std::sync::Mutex;

#[derive(Default)]
struct MemberWrites {
    pending: HashSet<i32>,
    conflicts: usize,
}

pub(super) struct Fixture {
    pub admin: Client,
    pub store: MongoSandboxStore,
    pub independent: MongoAgentOwnership,
    pub authority: Arc<Authority>,
    pub agent: Agent,
    pub name: String,
    app: String,
    commands: Arc<Mutex<Vec<Document>>>,
    member_writes: Arc<Mutex<MemberWrites>>,
}

pub(super) fn member(id: &str) -> Member {
    Member {
        instance_id: id.to_owned(),
        protocol: PROTOCOL_VERSION,
        relay_address: format!("http://{id}:8080"),
        expires_at_ms: 0,
        draining: false,
        owned_agents: 0,
        max_owned_agents: DEFAULT_MAX_OWNED_AGENTS,
    }
}

impl Fixture {
    pub async fn new() -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
            .expect("a disposable failpoint-enabled replica set is required");
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let app = format!("m4-owner-fence-{suffix}");
        let name = format!("m4_owner_fence_{suffix}");
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let seen = commands.clone();
        let member_writes = Arc::new(Mutex::new(MemberWrites::default()));
        let writes = member_writes.clone();
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let mut writes = writes.lock().unwrap();
            match event {
                CommandEvent::Started(event) => {
                    if matches!(event.command.get_str("update"), Ok(MEMBERS)) {
                        writes.pending.insert(event.request_id);
                    }
                    seen.lock().unwrap().push(event.command);
                }
                CommandEvent::Failed(event) if writes.pending.remove(&event.request_id) => {
                    use mongodb::error::{ErrorKind, WriteFailure};
                    if matches!(event.failure.kind.as_ref(), ErrorKind::Command(error) if error.code == 112)
                        || matches!(event.failure.kind.as_ref(), ErrorKind::Write(WriteFailure::WriteError(error)) if error.code == 112)
                    {
                        writes.conflicts += 1;
                    }
                }
                CommandEvent::Succeeded(event) if writes.pending.remove(&event.request_id) => {
                    if event.reply.get_array("writeErrors").is_ok_and(|errors| {
                        errors.iter().any(|error| {
                            error
                                .as_document()
                                .is_some_and(|error| matches!(error.get_i32("code"), Ok(112)))
                        })
                    }) {
                        writes.conflicts += 1;
                    }
                }
                _ => {}
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        let (_, agent) = fixture(&store, "owned-effect").await;
        let store = store
            .enable_agent_ownership("original".into())
            .await
            .unwrap();
        let independent = MongoAgentOwnership::open(&admin, &name).await.unwrap();
        // Membership/acquisition use the independent client. Faults can affect
        // only the actual Agent publication, never manufacture its ownership.
        let coordinator =
            OwnershipCoordinator::register(Arc::new(independent.clone()), member("original"))
                .await
                .unwrap();
        let authority = coordinator.acquire(&agent.id).await.unwrap().unwrap();
        commands.lock().unwrap().clear();
        *member_writes.lock().unwrap() = MemberWrites::default();
        Self {
            admin,
            store,
            independent,
            authority,
            agent,
            name,
            app,
            commands,
            member_writes,
        }
    }

    pub fn member_write_conflicts(&self) -> usize {
        self.member_writes.lock().unwrap().conflicts
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub async fn fault(&self, command: &str, count: i32, unknown: bool, delay: bool) -> i64 {
        self.commands.lock().unwrap().clear();
        let mut data = doc! {
            "appName": &self.app, "failCommands": [command],
            "errorCode": if unknown {1_i32} else {112_i32},
            "errorLabels": if unknown {vec!["TransientTransactionError", "UnknownTransactionCommitResult"]}
                else {vec!["TransientTransactionError"]},
        };
        if delay {
            data.insert("blockConnection", true);
            data.insert("blockTimeMS", 2000_i32);
        }
        let result = self
            .admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand",
                "mode": if count == 0 {mongodb::bson::Bson::String("alwaysOn".into())}
                    else {mongodb::bson::Bson::Document(doc! {"times": count})},
                "data": data,
            })
            .await
            .unwrap();
        result
            .get_i64("count")
            .or_else(|_| result.get_i32("count").map(i64::from))
            .expect("Mongo failpoint count must be an integer")
    }

    pub async fn wait_for_fault(&self, prior: i64) {
        self.admin
            .database("admin")
            .run_command(doc! {
                "waitForFailPoint": "failCommand", "timesEntered": prior + 1,
                "maxTimeMS": 5000_i32,
            })
            .await
            .expect("the real ownership fence must reach the injected fault");
    }

    pub async fn clear(&self) -> Vec<Document> {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .expect("disable the app-scoped fault before assertions");
        let commands = self.commands.lock().unwrap().clone();
        let mut sessions = Vec::new();
        for command in &commands {
            if let Ok(session) = command.get_document("lsid") {
                if !sessions.contains(session) {
                    sessions.push(session.clone());
                }
            }
        }
        if !sessions.is_empty() {
            self.admin
                .database("admin")
                .run_command(doc! {"killSessions": sessions})
                .await
                .expect("release only sessions used by this fixture");
        }
        commands
    }

    pub async fn snapshot(&self) -> (Agent, i64, i64) {
        let agent = self.store.get_agent(&self.agent.id).await.unwrap().unwrap();
        let owner = self
            .independent
            .owners
            .find_one(doc! {"_id": self.agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let member = self
            .independent
            .members
            .find_one(doc! {"_id": "original"})
            .await
            .unwrap()
            .unwrap();
        (
            agent,
            member.get_i64("effect_revision").unwrap_or(0),
            owner.get_i64("effect_revision").unwrap_or(0),
        )
    }

    pub async fn cleanup(&self) {
        self.admin
            .database(&self.name)
            .drop()
            .await
            .expect("drop fixture before final assertions");
    }
}
