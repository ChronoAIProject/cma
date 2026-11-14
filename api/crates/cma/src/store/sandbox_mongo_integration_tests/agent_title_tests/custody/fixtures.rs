use super::*;
use crate::sandbox::titles::HttpAgentTitleGenerator;
use axum::response::IntoResponse;
use axum::{extract::State, routing::post, Json, Router};
use layer3_adapter::provider::ProviderEndpoint;

pub(super) struct Fixture {
    pub client: Client,
    pub name: String,
    pub a: Arc<MongoSandboxStore>,
    pub b: Arc<MongoSandboxStore>,
    pub service_a: Arc<AgentTitleService>,
    pub service_b: Arc<AgentTitleService>,
    pub id: AgentId,
}
impl Fixture {
    pub async fn new(suffix: &str) -> Self {
        let (client, name) = database(&format!("title_custody_{suffix}")).await;
        let a = Arc::new(
            MongoSandboxStore::open(&client, &name)
                .await
                .unwrap()
                .with_title_effect_custody(true),
        );
        let b = Arc::new(peer(&name, true).await);
        let id = create_eligible_agent(&a, "agt_title_custody").await;
        let service_a = AgentTitleService::new(a.clone(), "instance-a").unwrap();
        let service_b = AgentTitleService::new(b.clone(), "instance-b").unwrap();
        Self {
            client,
            name,
            a,
            b,
            service_a,
            service_b,
            id,
        }
    }
    pub async fn row(&self) -> Document {
        self.a
            .agents
            .find_one(doc! {"_id": self.id.as_str()})
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn intent(&self) -> (AgentTitleClaim, AgentTitleEffectIntent) {
        let claim = self
            .a
            .acquire_agent_title_claim(
                &AgentTitleClaimRequest::new(self.id.clone(), "origin"),
                SystemTime::now(),
            )
            .await
            .unwrap()
            .unwrap();
        let transcript = self
            .a
            .agent_title_transcript(&self.id)
            .await
            .unwrap()
            .unwrap();
        let intent = AgentTitleEffectIntent::new(&claim, &transcript, &Descriptor).unwrap();
        (claim, intent)
    }
    pub async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}
pub(super) async fn peer(name: &str, enabled: bool) -> MongoSandboxStore {
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    MongoSandboxStore::open(&client, name)
        .await
        .unwrap()
        .with_title_effect_custody(enabled)
}
pub(super) fn context() -> AgentTitleGenerationContext {
    AgentTitleGenerationContext::new("title-model", "test-title-credential").unwrap()
}
pub(super) async fn expire(store: &MongoSandboxStore, id: &AgentId) {
    // Move the complete lease interval into the past; keep its exact duration.
    // This simulates elapsed time without sleeping or changing host clocks.
    store
        .agents
        .update_one(
            doc! {"_id": id.as_str()},
            doc! {"$set": {
                "title_lease_acquired_at": mongodb::bson::DateTime::from_millis(1000),
                "title_lease_expires_at": mongodb::bson::DateTime::from_millis(46000),
                "title_next_attempt_at": mongodb::bson::DateTime::from_millis(0),
            }},
        )
        .await
        .unwrap();
}
struct Descriptor;
#[async_trait::async_trait]
impl PreparedAgentTitle for Descriptor {
    fn request_digest(&self) -> &str {
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
    fn strategy(&self) -> &str {
        "fixture-title-v1"
    }
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        panic!("store-only descriptor cannot send")
    }
}
pub(super) struct Provider {
    count: Arc<AtomicUsize>,
    server: tokio::task::JoinHandle<()>,
    pub generator: HttpAgentTitleGenerator,
}
impl Provider {
    pub async fn new(lost_response: bool) -> Self {
        let count = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = ProviderEndpoint {
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
        };
        let router = Router::new()
            .route(
                "/v1/responses",
                post(
                    |State((count, lost)): State<(Arc<AtomicUsize>, bool)>,
                     Json(_body): Json<serde_json::Value>| async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        if lost {
                            // The provider accepted the full request; its response
                            // stream fails before a usable result reaches the caller.
                            axum::response::Response::builder()
                                .header("content-type", "application/json")
                                .body(axum::body::Body::from_stream(futures_util::stream::once(
                                    async {
                                        Err::<String, _>(std::io::Error::other(
                                            "accepted response lost",
                                        ))
                                    },
                                )))
                                .unwrap()
                        } else {
                            Json(serde_json::json!({"output_text": "Retained provider title"}))
                                .into_response()
                        }
                    },
                ),
            )
            .with_state((count.clone(), lost_response));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            count,
            server,
            generator: HttpAgentTitleGenerator::new(&endpoint, "title-model").unwrap(),
        }
    }
    pub fn calls(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}
impl Drop for Provider {
    fn drop(&mut self) {
        self.server.abort();
    }
}
