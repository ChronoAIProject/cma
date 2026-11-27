use std::io::Write as _;

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::post,
    Router,
};
use layer2_core::profiles::{
    store::InMemoryProfileStore, AgentProfile, AgentProfileRef, AgentProfileStore,
    ProfileVisibility, SandboxProfileRef, WorkspaceProfileRef,
};
use layer2_core::sandbox::store::PageRequest;
use layer2_core::sandbox::{AgentContextService, SandboxPolicy, SandboxService, WorkspaceService};
use layer2_core::{Authorization, Capability, PlatformDefaults};
use serde_json::{json, Value};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::*;
use crate::projections::{AgentProjectionServices, UnavailableSandboxResources};

mod approval_tests;
mod binding_tests;
mod bootstrap_tests;
mod cancellation_tests;
mod cross_interface_tests;
mod mongo_tests;
mod receiver_handoff_tests;
mod runtime_tests;
mod task_home_tests;
mod terminal_failure_tests;
mod v2_tests;

const TEST_BEARER: &str = "Bearer local-sdk-conformance";

#[tokio::test]
#[ignore = "runs a loopback-only fixture for the official Responses SDK scripts"]
async fn official_responses_sdk_server() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("layer1_public::responses::v2=debug")
        .try_init();
    let port = std::env::var("CMA_SDK_SERVER_PORT")
        .map(|value| value.parse::<u16>().expect("CMA_SDK_SERVER_PORT is a port"))
        .unwrap_or(0);
    let server = start_server(port).await;
    println!("CMA_SDK_BASE_URL={}", server.base_url);
    std::io::stdout().flush().unwrap();
    server.task.await.unwrap();
}

#[tokio::test]
async fn official_sdk_fixture_uses_real_context_operations_and_live_recording() {
    let server = start_server(0).await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    assert_eq!(
        client
            .post(format!("{}/conversations", server.base_url))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let conversation: Value = client
        .post(format!("{}/conversations", server.base_url))
        .header("authorization", TEST_BEARER)
        .json(&json!({}))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = conversation["id"].as_str().unwrap();
    assert!(id.starts_with("agt_"));
    let response = client
        .post(format!("{}/responses", server.base_url))
        .header("authorization", TEST_BEARER)
        .json(&json!({"conversation": id, "input": "sdk-raw-with-tools", "stream": true}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let response = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{response}");
    for expected in [
        "response.created",
        "response.reasoning_summary_text.delta",
        "Checking the proof",
        "response.cma_chat.tool_activity",
        "Hello from CMA",
        "response.completed",
    ] {
        assert!(
            response.contains(expected),
            "missing {expected}: {response}"
        );
    }
    let conflict = client
        .post(format!("{}/responses", server.base_url))
        .header("authorization", TEST_BEARER)
        .json(&json!({"conversation": id, "input": "Must not fork", "stream": true}))
        .send()
        .await
        .unwrap();
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let items: Value = client
        .get(format!("{}/conversations/{id}/items", server.base_url))
        .header("authorization", TEST_BEARER)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!items.to_string().contains("Checking the proof"));
    assert!(items.to_string().contains("Hello from CMA"));
    client
        .post(server.shutdown_url())
        .header("authorization", TEST_BEARER)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    server.task.await.unwrap();
}

struct TestServer {
    base_url: String,
    task: JoinHandle<()>,
}

impl TestServer {
    fn shutdown_url(&self) -> String {
        format!(
            "{}/__sdk/shutdown",
            self.base_url.strip_suffix("/api/v1").unwrap()
        )
    }
}

async fn start_server(port: u16) -> TestServer {
    let harness = Arc::new(Harness::new(Arc::new(EmptyAgentSkillResolver)).await);
    let (router, command_worker) = conversation_router(&harness).await;
    let (shutdown, receiver) = oneshot::channel();
    let shutdown = Arc::new(Mutex::new(Some(shutdown)));
    let router = router
        .route(
            "/__sdk/shutdown",
            post(move || {
                let shutdown = shutdown.clone();
                async move {
                    if let Some(sender) = shutdown.lock().unwrap().take() {
                        let _ = sender.send(());
                    }
                    StatusCode::OK
                }
            }),
        )
        .layer(middleware::from_fn_with_state(
            harness.workspace.owner.clone(),
            authenticate,
        ));
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap();
    let base_url = format!("http://{}/api/v1", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut driver = tokio::spawn(drive_agent_readiness(harness));
        let (command_shutdown, command_receiver) = tokio::sync::watch::channel(false);
        let commands =
            crate::commands::CommandScheduler::new(command_worker).spawn(command_receiver);
        let server = axum::serve(listener, router).with_graceful_shutdown(async {
            let _ = receiver.await;
        });
        tokio::select! {
            result = server => result.unwrap(),
            result = &mut driver => panic!("SDK fixture worker stopped: {result:?}"),
            _ = tokio::time::sleep(Duration::from_secs(600)) => {},
        }
        let _ = command_shutdown.send(true);
        commands.await.unwrap();
        driver.abort();
        let _ = driver.await;
    });
    TestServer { base_url, task }
}

async fn authenticate(
    axum::extract::State(owner): axum::extract::State<SubjectId>,
    mut request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(TEST_BEARER)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(owner.clone()));
    request.extensions_mut().insert(v2_tests::context(&owner));
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "local-sdk-conformance".into(),
        ));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    Ok(next.run(request).await)
}

async fn conversation_router(
    harness: &Harness,
) -> (
    Router,
    Arc<layer2_core::sandbox::agent_io::AgentCommandWorker>,
) {
    conversation_router_with_probe(harness, None, "official-sdk-command-worker").await
}

async fn conversation_router_with_probe<S: FixtureStore>(
    harness: &Harness<S>,
    probe: Option<Arc<runtime_tests::RuntimeProbe>>,
    worker_holder: &str,
) -> (
    Router,
    Arc<layer2_core::sandbox::agent_io::AgentCommandWorker>,
) {
    let (services, context) = projection_components(harness, probe).await;
    v2_tests::compose(harness, services, context, worker_holder)
}
async fn projection_components<S: FixtureStore>(
    harness: &Harness<S>,
    probe: Option<Arc<runtime_tests::RuntimeProbe>>,
) -> (AgentProjectionServices, Arc<AgentContextService>) {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let snapshot = agent_profile(Vec::new());
    let stamp = rfc3339_at(harness.now);
    profiles
        .create_agent_profile(AgentProfile {
            internal: false,
            id: snapshot.id.clone(),
            owner: harness.workspace.owner.clone(),
            name: "SDK conformance".into(),
            description: String::new(),
            visibility: ProfileVisibility::Private,
            forked_from: None,
            uses_count: 0,
            draft: snapshot.content,
            draft_version: 1,
            created_at: stamp.clone(),
            updated_at: stamp.clone(),
            retired_at: None,
        })
        .await
        .unwrap();
    profiles
        .publish_agent_profile(&snapshot.id, 1, stamp)
        .await
        .unwrap();
    let sandbox = harness
        .store
        .get_sandbox(&harness.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let defaults = PlatformDefaults {
        sandbox_profile: SandboxProfileRef::new(sandbox.profile.id, sandbox.profile.revision),
        workspace_profile: WorkspaceProfileRef::new(
            harness.workspace.profile.id.clone(),
            harness.workspace.profile.revision,
        ),
        agent_profile: AgentProfileRef::new(snapshot.id, 1),
    };
    let registry = match probe {
        Some(probe) => runtime_tests::SdkRegistry::with_probe(
            harness.registry.clone(),
            harness.updates.clone(),
            Some(probe),
        ),
        None => runtime_tests::SdkRegistry::new(harness.registry.clone(), harness.updates.clone()),
    };
    let services = AgentProjectionServices::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        harness.store.clone(),
        profiles.clone(),
        registry,
        Arc::new(UnavailableSandboxResources),
        RuntimeCapabilities::new([RuntimeCapability::Conversation]),
    );
    let context = AgentContextService::new(
        SandboxService::new(
            harness.store.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(harness.store.clone(), profiles.clone()),
        services.agents(),
        profiles,
        Arc::new(defaults),
    );
    (services, context)
}

async fn drive_agent_readiness(harness: Arc<Harness>) {
    loop {
        let agents = harness
            .store
            .list_agents(&harness.workspace.owner, &PageRequest::default())
            .await
            .unwrap();
        assert!(agents.next.is_none(), "SDK fixture exceeded one owner page");
        for agent in agents.items {
            if agent.status == AgentStatus::Starting
                || agent.work.step == AgentWorkStep::PublishConnection
            {
                harness
                    .worker
                    .run_one(&agent.id, SystemTime::now())
                    .await
                    .unwrap();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
