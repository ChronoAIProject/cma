use super::*;
use crate::sandbox::suggestions::HttpSuggestionGenerator;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::post,
    Json, Router,
};
use layer2_core::sandbox::suggestions::*;
use layer2_core::sandbox::{
    AgentOperationService, AgentReadService, AgentRuntimeConnection, AgentRuntimeError,
    AgentRuntimeRegistry, AgentRuntimeTarget, AgentService, SandboxResourceReadError,
    SandboxResourceReader, SandboxResourceReading,
};
use tower::ServiceExt;

mod completed_operation;
mod completion_contention;
mod context_contention;
mod lifecycle;
mod parent_cutoff;
#[cfg(feature = "real-model-suggestion-samples")]
mod samples;
mod writer_admission;

struct NoRuntime;
#[async_trait::async_trait]
impl layer2_core::OperationStore for NoRuntime {
    async fn reserve(
        &self,
        _: &layer2_core::OperationClaim,
    ) -> Result<layer2_core::OperationReservation, layer2_core::StoreError> {
        panic!("suggestion reads must not reserve commands");
    }
    async fn complete(
        &self,
        _: &layer2_core::OperationClaim,
        _: Vec<u8>,
    ) -> Result<(), layer2_core::StoreError> {
        panic!("suggestion reads must not complete commands");
    }
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for NoRuntime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        panic!("suggestions must not contact an Agent runtime");
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("suggestions must not fence Agent runtime input");
    }
}
#[async_trait::async_trait]
impl SandboxResourceReader for NoRuntime {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("suggestion reads must not inspect runtime resources");
    }
}

fn router(store: Arc<MongoSandboxStore>) -> Router {
    let runtime = Arc::new(NoRuntime);
    let profiles = Arc::new(layer2_core::profiles::store::InMemoryProfileStore::default());
    let agents = AgentService::new(store.clone(), profiles, runtime.clone(), Default::default());
    let reads = AgentReadService::new(store.clone(), runtime.clone());
    let operations = AgentOperationService::new(store.clone(), runtime);
    layer1_public::rest::agent_router(
        layer1_public::rest::AgentRoutesState::new(
            layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
            agents,
            reads,
            operations,
            Arc::new(NoRuntime),
        )
        .with_suggestions(store),
    )
}

async fn read(router: &Router, agent: &Agent, owner: &str) -> (StatusCode, serde_json::Value) {
    let mut request = Request::builder()
        .uri(format!(
            "/api/v1/agents/{}/suggestions?turn_sequence=3&answer_sequence=5",
            agent.id
        ))
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(SubjectId(owner.into())));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    let response = router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 32 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

async fn seed(
    store: &MongoSandboxStore,
    suffix: &str,
    goal: &str,
    answer: &str,
) -> (Sandbox, Agent) {
    let parent = active_sandbox(&format!("sbx_{suffix}"));
    let mut agent = ready_agent(&parent, &format!("agt_{suffix}"));
    agent.conversation.last_seq = 0;
    store.create_sandbox(parent.clone()).await.unwrap();
    store
        .create_workspace(ready_workspace(
            &parent,
            agent.workspace_id.as_str(),
            "project",
        ))
        .await
        .unwrap();
    let (mut worktree, _) = worktree_and_agent(&parent.id);
    worktree.id = agent.worktree_id.clone();
    worktree.workspace_id = agent.workspace_id.clone();
    worktree.agent_id = Some(agent.id.clone());
    store.create_worktree(worktree).await.unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    for kind in [
        AgentLogKind::Created {
            workspace_id: agent.workspace_id.clone(),
            worktree_id: agent.worktree_id.clone(),
        },
        AgentLogKind::ConversationOpened {
            conversation_id: agent.conversation.conversation_id.clone().unwrap(),
            runtime_generation: 1,
        },
        AgentLogKind::TurnStarted {
            turn_sequence: 3,
            origin_key: None,
            model: None,
        },
        AgentLogKind::OwnerMessage {
            content: goal.into(),
        },
        AgentLogKind::AgentMessage {
            content: answer.into(),
        },
    ] {
        store
            .append_agent_log(&agent.id, NOW.into(), kind)
            .await
            .unwrap();
    }
    let previous = agent.revision;
    agent.revision += 1;
    agent.conversation.last_seq = 5;
    agent.conversation.active_turn = Some(3);
    agent.status = AgentStatus::Working;
    store.save_agent(previous, agent.clone()).await.unwrap();
    use layer2_core::sandbox::{
        AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
    };
    DurableAgentUpdateRecorder::new(Arc::new(store.clone()))
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            layer3_adapter::ConversationUpdate::TurnEnded(layer3_adapter::PromptResponse::new(
                layer3_adapter::TurnStopReason::EndTurn,
            )),
            NOW.into(),
        )
        .await
        .unwrap();
    let agent = store.get_agent(&agent.id).await.unwrap().unwrap();
    (parent, agent)
}

fn response(output: &str) -> serde_json::Value {
    serde_json::json!({"status":"completed","model":"bounded-fixture","output":[{"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":output}]}],"usage":{"input_tokens":123,"output_tokens":24,
        "input_tokens_details":{"cached_tokens":100},"cost":{"amount":"0.000123","currency":"USD"}}})
}

#[tokio::test]
#[ignore = "requires an isolated Mongo replica set"]
async fn contextual_suggestions_mongo_saves_exact_generated_text_and_reopens_without_billing_again()
{
    let (client, name) = database("ticket13_saved").await;
    let store = Arc::new(enabled_store(&client, &name).await);
    let (_, agent) = seed(
        &store,
        "saved",
        "Compare SQLite and PostgreSQL for local storage.",
        "SQLite suits a local-only app.",
    )
    .await;
    let routes = router(store.clone());
    assert_eq!(read(&routes, &agent, "owner").await.1["status"], "pending");
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = calls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/responses", post(move |headers: axum::http::HeaderMap, Json(body): Json<serde_json::Value>| {
        let captured = captured.clone();
        async move {
            assert_eq!(headers["authorization"], "Bearer synthetic-suggestion-authority");
            assert_eq!(body["tools"], serde_json::json!([]));
            assert_eq!(body["store"], false);
            assert_eq!(body["max_output_tokens"], 1024);
            captured.fetch_add(1, Ordering::SeqCst);
            Json(response(r#"{"suggestions":[{"text":"Explain SQLite backup options.","evidence":"local-only app"}]}"#))
        }
    }))).await.unwrap();
    });
    let generator = HttpSuggestionGenerator::new(
        &endpoint,
        "bounded-fixture",
        SecretValue::new("synthetic-suggestion-authority".into()),
        None,
    )
    .unwrap();
    assert!(SuggestionGenerationService::new(store.clone())
        .run_next(&generator)
        .await
        .unwrap());
    let (status, first) = read(&routes, &agent, "owner").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(first["status"], "ready", "{first}");
    assert_eq!(
        first["set"]["items"][0]["text"],
        "Explain SQLite backup options."
    );
    assert_eq!(first["observation"]["usage"]["input_tokens"], 123);
    assert_eq!(first["observation"]["usage"]["cost"]["amount"], "0.000123");
    let replica = Arc::new(enabled_store(&client, &name).await);
    assert!(!SuggestionGenerationService::new(replica.clone())
        .run_next(&generator)
        .await
        .unwrap());
    assert_eq!(read(&router(replica), &agent, "owner").await.1, first);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        read(&routes, &agent, "someone-else").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        store
            .agent_log(&agent.id, 0, 100)
            .await
            .unwrap()
            .iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::TurnStarted { .. }))
            .count(),
        1
    );
    server.abort();
    client.database(&name).drop().await.unwrap();
}

async fn enabled_store(client: &mongodb::Client, name: &str) -> MongoSandboxStore {
    let mut store = MongoSandboxStore::open(client, name).await.unwrap();
    store.writer_policy = Arc::new(
        serde_json::from_value(serde_json::json!({
            "mode":"accounts", "accounts":["owner"], "capabilities":["suggested_followup_admission"]
        }))
        .unwrap(),
    );
    store
}
