use super::*;
use layer2_core::sandbox::AgentReadError;
use mongodb::{event::EventHandler, options::ClientOptions};

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub store: Arc<MongoSandboxStore>,
    pub other: Arc<MongoSandboxStore>,
    pub parent: Sandbox,
    pub agent: Agent,
    pub claim: SuggestionWork,
    pub gate: Arc<Gate>,
    pub app: String,
}

impl Fixture {
    pub async fn new() -> Self {
        let (client, name) = database("suggestion_parent").await;
        let gate = Arc::new(Gate::default());
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let app = format!("suggestion-parent-{}", uuid::Uuid::new_v4().simple());
        options.app_name = Some(app.clone());
        let capture = gate.clone();
        options.command_event_handler =
            Some(EventHandler::callback(move |event| capture.observe(event)));
        let observed = Client::with_options(options).unwrap();
        let store = Arc::new(enabled_store(&observed, &name).await);
        let other = Arc::new(enabled_store(&client, &name).await);
        let (parent, agent) = seed(
            &store,
            "parent_cutoff",
            "Compare local databases.",
            "SQLite suits a local-only app.",
        )
        .await;
        let claim = store
            .claim_suggestions(unix_millis())
            .await
            .unwrap()
            .unwrap();
        Self {
            client,
            name,
            store,
            other,
            parent,
            agent,
            claim,
            gate,
            app,
        }
    }

    pub fn completion(&self) -> SuggestionCompletion {
        SuggestionCompletion {
            status: SuggestionStatus::Ready,
            set: Some(SuggestedReplySet {
                id: self.claim.id.clone(),
                viewer_id: self.claim.owner.to_string(),
                agent_id: self.agent.id.to_string(),
                answer_id: "event:5".into(),
                scope: self.claim.scope,
                source: SuggestionSource {
                    kind: "generated".into(),
                    version: STRATEGY_VERSION.into(),
                },
                items: vec![SuggestedReply {
                    id: "choice".into(),
                    text: "Explain SQLite backups.".into(),
                    rank: 0,
                }],
            }),
            observation: SuggestionObservation {
                elapsed_ms: 1,
                usage: SuggestionUsage {
                    input_tokens: Some(23),
                    ..Default::default()
                },
                failure: None,
            },
        }
    }

    pub fn complete(&self) -> tokio::task::JoinHandle<Result<(), AgentReadError>> {
        let (store, claim, completion) =
            (self.store.clone(), self.claim.clone(), self.completion());
        tokio::spawn(async move { store.complete_suggestions(&claim, completion).await })
    }

    pub async fn pause(
        &self,
        store: &MongoSandboxStore,
    ) -> Result<SandboxLifecycleCommandResult, SandboxStoreError> {
        store
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                self.parent.id.clone(),
                self.parent.owner.clone(),
                NOW.into(),
                None,
            ))
            .await
    }

    pub async fn row(&self) -> Document {
        self.other
            .suggestions
            .find_one(doc! {"_id":&self.claim.id})
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn assert_hidden(&self) {
        let reading = read(&router(self.other.clone()), &self.agent, "owner")
            .await
            .1;
        assert_eq!(reading["status"], "expired");
        assert!(reading["set"].is_null());
    }

    pub async fn cleanup(&self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
