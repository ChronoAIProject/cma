//! The same synthetic failures exercise the current base and consent-intent repair.
use super::*;
use mongodb::{
    bson::Document,
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Arc, Mutex};

struct Fixture {
    admin: Client,
    database: String,
    app: String,
    store: MongoSandboxStore,
    observer: Arc<MongoSandboxStore>,
    seed: AgentAccessRecord,
    commands: Arc<Mutex<Vec<Document>>>,
}

impl Fixture {
    async fn new() -> Self {
        let (_seed_client, database, observer, seed) = tests::approved_fixture().await;
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let app = format!("consent-admission-{}", uuid::Uuid::new_v4().simple());
        let mut options = ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let commands = Arc::new(Mutex::new(Vec::new()));
        let captured = commands.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.command.get_bool("startTransaction").ok() == Some(true)
                    || event.command.get_str("insert").ok() == Some(COLLECTION)
                {
                    captured.lock().unwrap().push(event.command);
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = MongoSandboxStore::open(&client, &database).await.unwrap();
        assert_eq!(
            store.agent_access_request(&seed.id).await.unwrap(),
            Some(seed.clone())
        );
        Self {
            admin,
            database,
            app,
            store,
            observer,
            seed,
            commands,
        }
    }

    async fn arm(&self, command: &str, mut fault: Document) {
        self.commands.lock().unwrap().clear();
        fault.insert("failCommands", vec![command]);
        fault.insert("appName", &self.app);
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": {"times": 1_i32}, "data": fault,
            })
            .await
            .unwrap();
    }

    async fn disarm(&self) -> Vec<Document> {
        self.admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .unwrap();
        std::mem::take(&mut *self.commands.lock().unwrap())
    }

    fn proposal(&self) -> AgentAccessProposal {
        AgentAccessProposal {
            request_key: "separate-consent-attempt".into(),
            ..self.seed.proposal.clone()
        }
    }

    async fn close(self) {
        self.admin.database(&self.database).drop().await.unwrap();
    }
}

fn starts(commands: &[Document]) -> usize {
    commands
        .iter()
        .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
        .count()
}

fn inserts(commands: &[Document]) -> Vec<&[mongodb::bson::Bson]> {
    commands
        .iter()
        .filter_map(|command| command.get_array("documents").ok().map(Vec::as_slice))
        .collect()
}

fn abort() -> Document {
    doc! {"errorCode": 112_i32, "errorLabels": ["TransientTransactionError"]}
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn consent_admission_retry_reuses_original_insert_bytes() {
    let fixture = Fixture::new().await;
    let mut fault = abort();
    // Hold the aborted insert across a clock second so recreated timestamps differ.
    fault.insert("blockConnection", true);
    fault.insert("blockTimeMS", 1200_i32);
    fixture.arm("insert", fault).await;
    let result = fixture
        .store
        .request_agent_access(fixture.seed.requester.clone(), fixture.proposal())
        .await;
    let commands = fixture.disarm().await;
    let request_id =
        records::request_id(&fixture.seed.requester, &fixture.proposal().request_key).unwrap();
    let stored = fixture
        .observer
        .agent_access_request(&request_id)
        .await
        .unwrap();
    fixture.close().await;
    let attempts = inserts(&commands);
    assert_eq!(starts(&commands), 2);
    assert_eq!(attempts.len(), 2);
    assert_eq!(stored, result.ok());
    assert!(stored.is_some());
    assert_eq!(attempts[0], attempts[1]);
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn consent_admission_retry_keeps_reads_and_authority_failures_distinct() {
    let fixture = Fixture::new().await;
    fixture.arm("aggregate", abort()).await;
    let requester = &fixture.seed.requester;
    let list = fixture
        .store
        .list_agent_access_requests(&requester.owner, &requester.agent_id)
        .await;
    let commands = fixture.disarm().await;
    let denied = fixture
        .store
        .list_agent_access_requests(&SubjectId::new("foreign-owner"), &requester.agent_id)
        .await;
    let refused_commands = fixture.disarm().await;
    let kept = fixture
        .observer
        .agent_access_request(&fixture.seed.id)
        .await
        .unwrap();
    let seed = fixture.seed.clone();
    fixture.close().await;
    assert_eq!(list, Ok(vec![seed.clone()]));
    assert_eq!(starts(&commands), 2);
    assert_eq!(denied, Err(AgentAccessError::NotFound));
    assert_eq!(starts(&refused_commands), 1);
    assert!(inserts(&refused_commands).is_empty());
    assert_eq!(kept, Some(seed));
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn consent_admission_retry_preserves_the_exact_requester_fence() {
    let fixture = Fixture::new().await;
    let requester = fixture.seed.requester.clone();
    fixture.arm("aggregate", abort()).await;
    let created = fixture
        .store
        .request_agent_access(requester.clone(), fixture.proposal())
        .await;
    let commands = fixture.disarm().await;
    let mut stale = requester.clone();
    stale.conversation = "obsolete-conversation".into();
    let mut another = fixture.proposal();
    another.request_key = "stale-consent".into();
    let denied = fixture
        .store
        .request_agent_access(stale.clone(), another.clone())
        .await;
    let refusals = fixture.disarm().await;
    let id = records::request_id(&stale, &another.request_key).unwrap();
    let stored = fixture.observer.agent_access_request(&id).await.unwrap();
    fixture.close().await;
    assert_eq!(created.unwrap().requester, requester);
    assert_eq!(starts(&commands), 2);
    assert_eq!(inserts(&commands).len(), 1);
    assert_eq!(denied, Err(AgentAccessError::NotFound));
    assert_eq!(starts(&refusals), 1);
    assert!(inserts(&refusals).is_empty());
    assert!(stored.is_none());
}

#[tokio::test]
#[ignore = "requires a disposable MongoDB replica set with failpoints"]
async fn consent_admission_retry_refuses_uncertain_duplicate_and_transient_labels() {
    let fixture = Fixture::new().await;
    fixture
        .arm(
            "insert",
            doc! {"errorCode": 11_000_i32,
                "errorLabels": ["TransientTransactionError", "UnknownTransactionCommitResult"],
            },
        )
        .await;
    let result = fixture
        .store
        .request_agent_access(fixture.seed.requester.clone(), fixture.proposal())
        .await;
    let commands = fixture.disarm().await;
    let id = records::request_id(&fixture.seed.requester, &fixture.proposal().request_key).unwrap();
    let stored = fixture.observer.agent_access_request(&id).await.unwrap();
    fixture.close().await;
    assert_eq!(result, Err(AgentAccessError::Unavailable));
    assert_eq!(starts(&commands), 1);
    assert_eq!(inserts(&commands).len(), 1);
    assert!(stored.is_none());
}
