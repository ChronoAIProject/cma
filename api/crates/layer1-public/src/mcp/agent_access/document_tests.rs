use super::*;
use layer2_core::document_artifacts::{
    memory::{MemoryDocumentBlobs, MemoryDocuments},
    DocumentService,
};

struct RuntimeAuth(Arc<Stub>);
#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for RuntimeAuth {
    async fn authenticate_setup(&self, value: &str) -> Result<(), AgentAccessError> {
        self.0.authenticate_setup(value).await
    }
    async fn authenticate(&self, value: &str) -> Result<AgentAccessRequester, AgentAccessError> {
        let mut identity = self.0.authenticate(value).await?;
        identity.agent_id = AgentId(format!("agt_{}", "a".repeat(32)));
        identity.creation.agent_id = identity.agent_id.clone();
        Ok(identity)
    }
}
fn fixture(stub: &Arc<Stub>) -> Router {
    let service = DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    );
    agent_tools_router(
        Arc::new(RuntimeAuth(stub.clone())),
        stub.clone(),
        Some(service),
        "https://cma.example",
    )
}
fn publish(markdown: String) -> Value {
    rpc(
        "tools/call",
        json!({"name":"publish_document","arguments":{"request_key":"write-a-spec","title":"Reviewed design","markdown":markdown}}),
    )
}
#[tokio::test]
async fn discovers_document_tools_without_granting_unconfirmed_mutation() {
    let stub = Stub::new(false);
    let app = fixture(&stub);
    let tools = body(
        app.clone()
            .oneshot(request(&stub, &rpc("tools/list", json!({}))))
            .await
            .unwrap(),
    )
    .await;
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "agent_key_scope_revision",
            "publish_document",
            "update_document"
        ]
    );
    let response = app
        .oneshot(request(&stub, &publish("# Spec".into())))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
#[tokio::test]
async fn publishes_large_private_document_replays_and_updates_with_cas() {
    let stub = Stub::new(true);
    let app = fixture(&stub);
    let command = publish(format!(
        "# Design\n{}",
        "A detailed paragraph.\n".repeat(1000)
    ));
    let first = body(app.clone().oneshot(request(&stub, &command)).await.unwrap()).await;
    assert_eq!(first["result"]["isError"], false, "{first}");
    let document = first["result"]["structuredContent"]["document"].clone();
    assert_eq!(document["owner_subject"], "owner");
    assert_eq!(document["access"], "owner");
    assert_eq!(
        first["result"]["structuredContent"]["url"],
        format!(
            "https://cma.example/artifacts/{}",
            document["id"].as_str().unwrap()
        )
    );
    let retry = body(app.clone().oneshot(request(&stub, &command)).await.unwrap()).await;
    assert_eq!(retry, first);
    let update = rpc(
        "tools/call",
        json!({"name":"update_document","arguments":{
        "request_key":"revise-spec","document_id":document["id"],"expected_revision":1,"markdown":"# Revised spec"}}),
    );
    let updated = body(app.clone().oneshot(request(&stub, &update)).await.unwrap()).await;
    assert_eq!(
        updated["result"]["structuredContent"]["document"]["revision"],
        2
    );
    let forbidden = rpc(
        "tools/call",
        json!({"name":"publish_document","arguments":{
        "request_key":"forge-owner","title":"Title","markdown":"Source","owner_subject":"another-user"}}),
    );
    assert_eq!(
        body(app.oneshot(request(&stub, &forbidden)).await.unwrap()).await["error"]["code"],
        -32602
    );
    assert!(stub.proposals.lock().unwrap().is_empty());
}
