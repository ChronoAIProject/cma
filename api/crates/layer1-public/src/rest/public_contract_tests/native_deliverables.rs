use super::runtime;
use crate::mcp::agent_delivery_tools_router;
use crate::rest::{agents_tests, answer_deliverables_router, shared_agent_tests};
use axum::{
    extract::State,
    http::{Method, StatusCode},
    routing::post,
    Extension, Json, Router,
};
use layer2_core::document_artifacts::{
    memory::{MemoryDocumentBlobs, MemoryDocuments},
    DocumentService,
};
use layer2_core::resource_access::{ResourceAccessService, ResourceOwner};
use layer2_core::sandbox::agent_access::{AgentAccessError, AgentAccessRequest};
use layer2_core::sandbox::agent_access_tool::{
    AgentAccessProposal, AgentAccessRequester, AgentAccessToolAdmission,
    AgentAccessToolAuthenticator,
};
use layer2_core::sandbox::agent_artifacts::AgentArtifactService;
use layer2_core::sandbox::agent_filesystem::AgentFileService;
use layer2_core::sandbox::answer_deliverables::AnswerDeliverableService;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore, InMemorySandboxStore, SandboxStore};
use layer2_core::sandbox::{
    AgentCreationOrigin, AgentCreationProvenance, AgentLogKind, AgentReadService, AgentStatus,
};
use layer2_core::{AgentId, Authorization, Caller, Capability, SandboxId};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tower::ServiceExt as _;

struct NativeAuthority {
    capability: String,
    requester: Mutex<Option<AgentAccessRequester>>,
}
#[async_trait::async_trait]
impl AgentAccessToolAuthenticator for NativeAuthority {
    async fn authenticate_setup(&self, capability: &str) -> Result<(), AgentAccessError> {
        if capability == self.capability {
            Ok(())
        } else {
            Err(AgentAccessError::Forbidden)
        }
    }
    async fn authenticate(
        &self,
        capability: &str,
    ) -> Result<AgentAccessRequester, AgentAccessError> {
        self.authenticate_setup(capability).await?;
        self.requester
            .lock()
            .unwrap()
            .clone()
            .ok_or(AgentAccessError::Forbidden)
    }
}
#[async_trait::async_trait]
impl AgentAccessToolAdmission for NativeAuthority {
    async fn request(
        &self,
        _: AgentAccessRequester,
        _: AgentAccessProposal,
    ) -> Result<AgentAccessRequest, AgentAccessError> {
        Err(AgentAccessError::Forbidden)
    }
}
struct Binding {
    store: Arc<InMemorySandboxStore>,
    reads: Arc<AgentReadService>,
    auth: Arc<NativeAuthority>,
    agent_id: String,
}
async fn bind(State(binding): State<Arc<Binding>>, Json(value): Json<Value>) -> Json<Value> {
    let conversation = value["conversation_id"].as_str().unwrap();
    uuid::Uuid::parse_str(conversation).unwrap();
    assert!(binding.auth.requester.lock().unwrap().is_none());
    let mut agent = agents_tests::seeded_agent(&binding.agent_id, AgentStatus::Ready, 2, None);
    agent.conversation.conversation_id = Some(conversation.to_owned());
    let agent = agents_tests::insert_agent(&binding.store, agent, vec![]).await;
    let caller = Caller::new(
        agent.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let lineage = binding.reads.lineage(&caller, &agent.id).await.unwrap();
    *binding.auth.requester.lock().unwrap() = Some(AgentAccessRequester {
        agent_id: agent.id.clone(),
        owner: agent.owner.clone(),
        sandbox_id: agent.sandbox_id.clone(),
        conversation: conversation.to_owned(),
        runtime: lineage.sandbox.runtime.unwrap(),
        creation: AgentCreationProvenance {
            schema_version: 1,
            agent_id: agent.id.clone(),
            agent_created_at: agent.created_at,
            sandbox_id: agent.sandbox_id,
            sandbox_created_at: lineage.sandbox.created_at,
            workspace_id: agent.workspace_id,
            worktree_id: agent.worktree_id,
            owner: ResourceOwner::Person {
                subject: agent.owner,
            },
            creation_origin: AgentCreationOrigin::Manual,
            created_by: None,
            provisioning_origin: agent.origin,
            creation_fingerprint: agent.creation_fingerprint,
        },
    });
    Json(json!({"bound": true}))
}

#[test]
#[ignore = "requires pinned Codex/ACP, an isolated Web server and Chrome; loopback model only"]
fn native_answer_deliverables_prove_real_file_document_receipts_and_public_downloads() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_stack_size(2 * 1024 * 1024)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(Box::pin(prove_native_deliveries()));
}

async fn prove_native_deliveries() {
    let binary = std::env::var("CMA_CODEX_TEST_BINARY").expect("pinned Codex required");
    let bridge = std::env::var("CMA_ACP_TEST_BRIDGE").expect("exact ACP bridge required");
    let web_url = std::env::var("CMA_NATIVE_WEB_URL").expect("isolated Web server required");
    let browser = std::env::var("CMA_E2E_BROWSER_PATH").expect("isolated browser binary required");
    let directory =
        std::env::temp_dir().join(format!("cma-ticket07-native-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&directory).unwrap();
    let fixture = agents_tests::fixture().await;
    let mut sandbox = fixture
        .store
        .get_sandbox(&SandboxId("sbx_route".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = sandbox.revision;
    sandbox.runtime.as_mut().unwrap().boot_id = Some("b".repeat(32));
    sandbox.lifecycle.home = Some(Box::new(runtime::persistent_home()));
    sandbox.revision += 1;
    fixture.store.save_sandbox(revision, sandbox).await.unwrap();
    let access = Arc::new(ResourceAccessService::new(
        fixture.store.clone(),
        Arc::new(shared_agent_tests::Authority(Mutex::new(
            shared_agent_tests::Reply::Member,
        ))),
    ));
    let mut files = runtime::Files::new("owner");
    files.root = Some(directory.clone());
    let documents = DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    );
    let service = AnswerDeliverableService::new(
        fixture.reads(),
        AgentArtifactService::new(fixture.store.clone(), access.clone()),
    )
    .with_files(AgentFileService::new(access, Arc::new(files)))
    .with_documents(documents.clone());
    let auth = Arc::new(NativeAuthority {
        capability: format!(
            "cma_at_{}",
            uuid::Uuid::new_v4().simple().to_string().repeat(2)
        ),
        requester: Mutex::new(None),
    });
    let agent_id = format!("agt_{}", "a".repeat(32));
    let binding = Arc::new(Binding {
        store: fixture.store.clone(),
        reads: fixture.reads(),
        auth: auth.clone(),
        agent_id: agent_id.clone(),
    });
    let app = agent_delivery_tools_router(
        auth.clone(),
        auth.clone(),
        Some(documents),
        Some(service.clone()),
        "http://127.0.0.1",
    )
    .merge(
        Router::new()
            .route("/proof/bind", post(bind))
            .with_state(binding),
    )
    .merge(
        fixture
            .router
            .clone()
            .merge(answer_deliverables_router(service.clone()))
            .layer(Extension(crate::AuthenticatedSubject(
                layer2_core::SubjectId::new("owner-a"),
            )))
            .layer(Extension(Authorization::new([Capability::UseOwnSandboxes]))),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/native_answer_deliverables.cjs");
    let output = tokio::process::Command::new("node")
        .arg(script)
        .arg(format!("http://{address}"))
        .arg(&directory)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("CMA_CODEX_TEST_BINARY", binary)
        .env("CMA_ACP_TEST_BRIDGE", bridge)
        .env("CMA_DELIVERY_CAPABILITY", &auth.capability)
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "native proof failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let observed: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(observed["stopReason"], "end_turn");
    let answer = observed["answer"].as_str().unwrap();
    assert_eq!(
        std::fs::read(directory.join("report.txt")).unwrap(),
        b"Native answer file\n"
    );
    let agent = AgentId(agent_id.clone());
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Deliver a file and a persistent document.".into(),
        },
        AgentLogKind::AgentMessage {
            content: answer.into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 3 },
    ] {
        fixture
            .store
            .append_agent_log(&agent, "2026-11-03T00:00:00Z".into(), kind)
            .await
            .unwrap();
    }
    let mut stored = fixture.store.get_agent(&agent).await.unwrap().unwrap();
    let revision = stored.revision;
    stored.conversation.last_seq = 6;
    stored.revision += 1;
    fixture.store.save_agent(revision, stored).await.unwrap();
    tokio::spawn(async move {
    let root = format!("/api/v1/agents/{agent_id}/answers/event:5/deliverables");
    let router = answer_deliverables_router(service.clone());
    let (status, listed) = agents_tests::json_response(&router, agents_tests::request(Method::GET, &root, "owner-a", None, None)).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{observed}");
    for item in items {
        let expected = match item["reference"]["resource"]["kind"].as_str().unwrap() {
            "live_file" => b"Native answer file\n".as_slice(),
            "document" => b"# Native document\n\nKnown persistent content.\n".as_slice(),
            other => panic!("unexpected delivery kind: {other}"),
        };
        let response = router.clone().oneshot(agents_tests::request(Method::GET, &format!("{root}/{}/download", item["id"].as_str().unwrap()), "owner-a", None, None)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(axum::body::to_bytes(response.into_body(), 1024).await.unwrap(), expected);
    }
    let (status, replay) = agents_tests::json_response(&answer_deliverables_router(service), agents_tests::request(Method::GET, &root, "owner-a", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay, listed);
    println!("native proof: pinned Codex={}, session={}, answer=event:5, turn=3, verified_resources=2, worker_stack=2097152, bridge_sha256={}", observed["version"], observed["sessionId"], observed["bridge_sha256"]);
    for item in items { println!("resource={}, bytes={}, sha256={}", item["reference"]["resource"]["kind"], item["reference"]["size_bytes"], item["reference"]["sha256"]); }
    }).await.unwrap();
    let web = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../web")
        .canonicalize()
        .unwrap();
    let browser_proof = tokio::process::Command::new("node")
        .arg(web.join("node_modules/@playwright/test/cli.js"))
        .args([
            "test",
            "--config",
            "e2e/answer-deliverables-native.config.ts",
        ])
        .current_dir(&web)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap())
        .env("HOME", &directory)
        .env("CMA_NATIVE_WEB_URL", web_url)
        .env("CMA_NATIVE_DELIVERY_ORIGIN", format!("http://{address}"))
        .env("CMA_E2E_BROWSER_PATH", browser)
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    server.abort();
    assert!(
        browser_proof.status.success(),
        "native browser proof failed: {}\n{}",
        String::from_utf8_lossy(&browser_proof.stdout),
        String::from_utf8_lossy(&browser_proof.stderr)
    );
    println!("{}", String::from_utf8_lossy(&browser_proof.stdout));
    std::fs::remove_dir_all(directory).unwrap();
}
