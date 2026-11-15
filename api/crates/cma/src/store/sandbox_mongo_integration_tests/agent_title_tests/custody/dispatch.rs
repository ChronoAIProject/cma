use super::*;
use layer2_core::sandbox::{AgentTitleGenerationRequest, AgentTitleGenerator};

struct LostAckGenerator<'a> {
    inner: &'a crate::sandbox::titles::HttpAgentTitleGenerator,
    admin: &'a Client,
    app: &'a str,
    successor: Arc<AgentTitleService>,
    id: AgentId,
}
#[async_trait::async_trait]
impl AgentTitleGenerator for LostAckGenerator<'_> {
    async fn generate(
        &self,
        _: AgentTitleGenerationRequest,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        panic!("custody must use preparation")
    }
    async fn prepare(
        &self,
        request: AgentTitleGenerationRequest,
    ) -> Result<Box<dyn PreparedAgentTitle>, AgentTitleGenerationError> {
        let inner = self.inner.prepare(request).await?;
        super::receipts::lost_update_ack(self.admin, self.app).await;
        Ok(Box::new(CompetingRediscovery {
            inner,
            successor: self.successor.clone(),
            id: self.id.clone(),
        }))
    }
}
struct CompetingRediscovery {
    inner: Box<dyn PreparedAgentTitle>,
    successor: Arc<AgentTitleService>,
    id: AgentId,
}
#[async_trait::async_trait]
impl PreparedAgentTitle for CompetingRediscovery {
    fn request_digest(&self) -> &str {
        self.inner.request_digest()
    }
    fn strategy(&self) -> &str {
        self.inner.strategy()
    }
    async fn dispatch(
        self: Box<Self>,
    ) -> Result<AgentTitleGenerationResponse, AgentTitleGenerationError> {
        assert_eq!(
            self.successor
                .generate_agent_title(&self.id, None, None)
                .await
                .unwrap(),
            AgentTitleOutcome::Skipped
        );
        self.inner.dispatch().await
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn title_custody_lost_intent_ack_only_origin_capability_may_make_first_post() {
    let f = Fixture::new("origin_capability").await;
    let provider = Provider::new(false).await;
    let mut options =
        ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
            .await
            .unwrap();
    let app = format!("title-origin-{}", uuid::Uuid::new_v4());
    options.app_name = Some(app.clone());
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(
        MongoSandboxStore::open(&client, &f.name)
            .await
            .unwrap()
            .with_title_effect_custody(true),
    );
    let service = AgentTitleService::new(store, "original-capability").unwrap();
    let generator = LostAckGenerator {
        inner: &provider.generator,
        admin: &f.client,
        app: &app,
        successor: f.service_b.clone(),
        id: f.id.clone(),
    };
    let result = service
        .generate_agent_title(&f.id, Some(&context()), Some(&generator))
        .await;
    disable_failpoint(&f.client).await;
    assert_eq!(result.unwrap(), AgentTitleOutcome::Generated);
    assert_eq!(provider.calls(), 1);
    assert_eq!(
        f.service_b
            .generate_agent_title(&f.id, Some(&context()), Some(&provider.generator))
            .await
            .unwrap(),
        AgentTitleOutcome::Skipped
    );
    assert_eq!(provider.calls(), 1);
    f.cleanup().await;
}
