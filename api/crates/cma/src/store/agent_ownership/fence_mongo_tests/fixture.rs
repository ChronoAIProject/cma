use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::fixture;
use layer2_core::agent_ownership::{Member, OwnershipCoordinator, DEFAULT_MAX_OWNED_AGENTS};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;
use std::sync::Mutex;

fn failpoint_count(result: &Document) -> Result<i64, &'static str> {
    let count = match result.get("count") {
        Some(mongodb::bson::Bson::Int32(value)) => i64::from(*value),
        Some(mongodb::bson::Bson::Int64(value)) => *value,
        _ => return Err("Mongo failpoint count must be a BSON integer"),
    };
    if count < 0 {
        return Err("Mongo failpoint count must be nonnegative");
    }
    Ok(count)
}

fn next_fault_count(prior: i64) -> Result<i64, &'static str> {
    if prior < 0 {
        return Err("Mongo failpoint count must be nonnegative");
    }
    prior.checked_add(1).ok_or("Mongo failpoint count overflow")
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
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                seen.lock().unwrap().push(event.command);
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
        Self {
            admin,
            store,
            independent,
            authority,
            agent,
            name,
            app,
            commands,
        }
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
        failpoint_count(&result).expect("valid Mongo failpoint count")
    }

    pub async fn wait_for_fault(&self, prior: i64) {
        let next = next_fault_count(prior).expect("representable next Mongo failpoint count");
        self.admin
            .database("admin")
            .run_command(doc! {
                "waitForFailPoint": "failCommand", "timesEntered": next,
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

#[test]
fn failpoint_counter_accepts_both_integer_widths_without_truncation() {
    use mongodb::bson::Bson;
    for (value, expected) in [
        (Bson::Int32(0), 0),
        (Bson::Int32(i32::MAX), i64::from(i32::MAX)),
        (Bson::Int64(0), 0),
        (
            Bson::Int64(i64::from(i32::MAX) + 1),
            i64::from(i32::MAX) + 1,
        ),
        (Bson::Int64(i64::MAX), i64::MAX),
    ] {
        assert_eq!(failpoint_count(&doc! {"count": value}), Ok(expected));
    }
}

#[test]
fn failpoint_counter_rejects_negative_missing_and_noninteger_values() {
    use mongodb::bson::Bson;
    assert!(failpoint_count(&Document::new()).is_err());
    for value in [
        Bson::Int32(-1),
        Bson::Int64(-1),
        Bson::Int64(i64::MIN),
        Bson::Double(1.0),
        Bson::Double(f64::NAN),
        Bson::String("1".into()),
        Bson::Boolean(true),
        Bson::Null,
        Bson::Array(Vec::new()),
        Bson::Document(Document::new()),
    ] {
        assert!(failpoint_count(&doc! {"count": value}).is_err());
    }
}

#[test]
fn failpoint_wait_target_increments_exactly_and_rejects_overflow() {
    assert_eq!(next_fault_count(0), Ok(1));
    assert_eq!(
        next_fault_count(i64::from(i32::MAX)),
        Ok(i64::from(i32::MAX) + 1)
    );
    assert_eq!(next_fault_count(i64::MAX - 1), Ok(i64::MAX));
    assert!(next_fault_count(i64::MAX).is_err());
    assert!(next_fault_count(-1).is_err());
}
