use super::*;
use crate::rest::{agent_models_router, AgentModelCatalog, AgentModelCatalogError, AgentModelView};
use std::sync::atomic::{AtomicUsize, Ordering};

struct Models(AtomicUsize);
#[async_trait]
impl AgentModelCatalog for Models {
    async fn models(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Vec<AgentModelView>, AgentModelCatalogError> {
        assert_eq!(sandbox.owner.as_str(), OWNER);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(vec![AgentModelView {
            id: "provider-model".into(),
            label: "Provider model".into(),
        }])
    }
}

#[tokio::test]
async fn model_catalogue_checks_agent_ownership_before_using_its_credential() {
    let fixture = fixture().await;
    let agent = seeded_agent("agt_models", AgentStatus::Ready, 0, None);
    fixture
        .store
        .create_worktree(seeded_worktree(&agent))
        .await
        .unwrap();
    fixture.store.create_agent(agent).await.unwrap();
    let catalog = Arc::new(Models(AtomicUsize::new(0)));
    let router = agent_models_router(fixture.state.reads.clone(), catalog.clone());
    let path = "/api/v1/agents/agt_models/models";
    for subject in [OTHER, "missing-user"] {
        let (status, _) =
            json_response(&router, request(Method::GET, path, subject, None, None)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
    assert_eq!(catalog.0.load(Ordering::SeqCst), 0);
    let response = router
        .oneshot(request(Method::GET, path, OWNER, None, None))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(
        body,
        json!({"models":[{"id":"provider-model","label":"Provider model"}]})
    );
    assert_eq!(catalog.0.load(Ordering::SeqCst), 1);
}
