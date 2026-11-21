use super::runtime;
use crate::mcp::agent_delivery_tools_router;
use crate::rest::{
    agent_filesystem_router, agents_tests, answer_deliverables_router, shared_agent_tests,
};
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
        // Match the debug test stack used by the other native runtime contracts.
        .thread_stack_size(32 * 1024 * 1024)
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
    let mut adapter = runtime::Files::new("owner");
    adapter.root = Some(directory.clone());
    let files = AgentFileService::new(access.clone(), Arc::new(adapter));
    let documents = DocumentService::new(
        Arc::new(MemoryDocuments::default()),
        Arc::new(MemoryDocumentBlobs::default()),
    );
    let service = AnswerDeliverableService::new(
        fixture.reads(),
        AgentArtifactService::new(fixture.store.clone(), access.clone()),
    )
    .with_files(files.clone())
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
            .merge(agent_filesystem_router(files))
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
    let expected_files = observed["files"].as_array().unwrap();
    assert!(expected_files
        .iter()
        .any(|file| file["path"] == "output/nested/report.csv"));
    assert!(expected_files
        .iter()
        .any(|file| file["path"] == "output/nested/readme.md"));
    let agent = AgentId(agent_id.clone());
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 3,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Deliver verified live files and a persistent document.".into(),
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
    verify_receipts(
        answer_deliverables_router(service),
        &agent_id,
        &directory,
        &observed,
    )
    .await;
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

async fn verify_receipts(
    router: Router,
    agent_id: &str,
    directory: &std::path::Path,
    observed: &Value,
) {
    use sha2::{Digest, Sha256};
    let root = format!("/api/v1/agents/{agent_id}/answers/event:5/deliverables");
    let listing = || agents_tests::request(Method::GET, &root, "owner-a", None, None);
    let (status, legacy) = agents_tests::json_response(&router, listing()).await;
    assert_eq!(status, StatusCode::OK);
    let (status, listed) = agents_tests::json_response(&router, native_media(listing())).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let items = listed["items"].as_array().unwrap();
    let files = observed["files"].as_array().unwrap();
    assert_eq!(
        items.len(),
        files.len() + 1,
        "a repeated receipt must not add a card"
    );
    let legacy_items = legacy["items"].as_array().unwrap();
    assert_eq!(legacy_items.len(), items.len());
    for item in items {
        let reference = &item["reference"];
        let legacy_item = legacy_items
            .iter()
            .find(|item_in_legacy| item_in_legacy["id"] == item["id"])
            .unwrap();
        let id = item["id"].as_str().unwrap();
        let target = format!("{root}/{id}");
        let (expected, media_type) = match reference["resource"]["kind"].as_str().unwrap() {
            "live_file" => {
                let path = reference["resource"]["path"].as_str().unwrap();
                let verified = files.iter().find(|file| file["path"] == path).unwrap();
                assert_eq!(reference["sha256"], verified["sha256"]);
                assert_eq!(reference["size_bytes"], verified["size_bytes"]);
                assert_eq!(reference["media_type"], verified["stored_media_type"]);
                (
                    std::fs::read(directory.join(path)).unwrap(),
                    verified["media_type"].as_str().unwrap(),
                )
            }
            "document" => (
                b"# Native document\n\nKnown persistent content.\n".to_vec(),
                "application/octet-stream",
            ),
            other => panic!("unexpected delivery kind: {other}"),
        };
        if reference["resource"]["kind"] == "live_file" {
            let mut projected = item.clone();
            projected["reference"]["media_type"] = json!("application/octet-stream");
            assert_eq!(legacy_item, &projected);
        }
        let download_path = format!("{target}/download");
        let download =
            |subject| agents_tests::request(Method::GET, &download_path, subject, None, None);
        let response = router
            .clone()
            .oneshot(native_media(download("owner-a")))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-type"], media_type);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        assert_eq!(
            response.headers()["content-length"],
            expected.len().to_string()
        );
        assert_eq!(
            response.headers()["etag"],
            format!("\"{}\"", reference["sha256"].as_str().unwrap())
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert!(response.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment;"));
        let bytes = axum::body::to_bytes(response.into_body(), 32 * 1024 * 1024)
            .await
            .unwrap();
        assert_eq!(&bytes[..], expected);
        assert_eq!(reference["sha256"], format!("{:x}", Sha256::digest(&bytes)));
        assert_eq!(reference["size_bytes"], bytes.len());
        if matches!(
            reference["name"].as_str(),
            Some("report.csv" | "report.zip")
        ) {
            let (status, reading) = agents_tests::json_response(
                &router,
                agents_tests::request(Method::GET, &target, "owner-a", None, None),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(&reading["item"], legacy_item);
            let response = router.clone().oneshot(download("owner-a")).await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()["content-type"],
                "application/octet-stream"
            );
            assert_eq!(
                axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap(),
                bytes
            );
        }
        assert_eq!(
            router
                .clone()
                .oneshot(download("other"))
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
        if reference["name"] == "report.json" {
            let path = directory.join("output/nested/report.json");
            std::fs::write(&path, b"{\"changed\":true}\n").unwrap();
            let (status, problem) = agents_tests::json_response(&router, download("owner-a")).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(problem["code"], "deliverable_changed");
            std::fs::remove_file(&path).unwrap();
            let (status, problem) = agents_tests::json_response(&router, download("owner-a")).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(problem["code"], "file_not_found");
            std::fs::write(&path, &expected).unwrap();
        }
        println!(
            "verified attachment {} ({} bytes)",
            reference["name"],
            bytes.len()
        );
    }
    assert_eq!(
        agents_tests::json_response(&router, native_media(listing())).await,
        (StatusCode::OK, listed)
    );

    let path = "output/nested/over-limit.csv";
    std::fs::File::create(directory.join(path))
        .unwrap()
        .set_len(layer3_adapter::MAX_AGENT_FILE_BYTES + 1)
        .unwrap();
    let (status, problem) = agents_tests::json_response(
        &router,
        agents_tests::request(
            Method::POST,
            &format!("/api/v1/agents/{agent_id}/deliverable-references"),
            "owner-a",
            None,
            Some(json!({"kind": "live_file", "path": path})),
        ),
    )
    .await;
    std::fs::remove_file(directory.join(path)).unwrap();
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(problem["code"], "file_too_large");
}

fn native_media(
    mut request: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    request.headers_mut().insert(
        "prefer",
        axum::http::HeaderValue::from_static("cma-native-attachment-media"),
    );
    request
}
