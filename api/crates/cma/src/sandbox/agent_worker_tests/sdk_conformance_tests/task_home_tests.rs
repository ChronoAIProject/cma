use super::*;
use crate::agent_lifecycle::{InlineAgentLifecycle, RetiredImageResolver};
use crate::store::{MongoAgentLifecycleStore, MongoProfileStore, MongoSandboxStore};
use authority::{task_experience_policy, PersonalAuthority};
use axum::body::{to_bytes, Body};
use layer2_core::profiles::ProfileCapacityBounds;
use layer2_core::resource_access::{
    CallerContext, MembershipError, OrganizationAuthority, OrganizationChoice,
    ResourceAccessService,
};
use tower::ServiceExt;

mod authority;
mod canary_desktop;
mod desktop;
mod desktop_native;
mod environment;
mod environment_workers;
mod integrated_tests;
mod machine_policy;
mod receipt_output;
mod recovery;
mod repository_connection_tests;
mod repository_marker;
mod restart;
mod store_retry;
mod subprocess;

struct UnusedCompatibilityLedger;

#[async_trait::async_trait]
impl layer2_core::OperationStore for UnusedCompatibilityLedger {
    async fn reserve(
        &self,
        _: &layer2_core::OperationClaim,
    ) -> Result<layer2_core::OperationReservation, layer2_core::StoreError> {
        panic!("lifecycle admission must use its durable domain ledger")
    }

    async fn complete(
        &self,
        _: &layer2_core::OperationClaim,
        _: Vec<u8>,
    ) -> Result<(), layer2_core::StoreError> {
        panic!("lifecycle admission must use its durable domain ledger")
    }
}

async fn router(
    harness: &Harness<MongoSandboxStore>,
    database: &mongodb::Database,
    database_client: &mongodb::Client,
) -> Router {
    router_for(harness, database, database_client, "owner", false).await
}

async fn router_for(
    harness: &Harness<MongoSandboxStore>,
    database: &mongodb::Database,
    database_client: &mongodb::Client,
    owner: &str,
    can_create: bool,
) -> Router {
    router_components(
        harness.store.clone(),
        harness.registry.clone(),
        database,
        database_client,
        owner,
        can_create,
    )
    .await
}

async fn router_components(
    store: Arc<MongoSandboxStore>,
    registry: Arc<LiveAgentRegistry>,
    database: &mongodb::Database,
    database_client: &mongodb::Client,
    owner: &str,
    can_create: bool,
) -> Router {
    router_with_connections(
        store,
        registry,
        database,
        database_client,
        owner,
        can_create,
        Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
    )
    .await
}

async fn router_with_connections(
    store: Arc<MongoSandboxStore>,
    registry: Arc<LiveAgentRegistry>,
    database: &mongodb::Database,
    database_client: &mongodb::Client,
    owner: &str,
    can_create: bool,
    connections: Arc<dyn layer2_core::sandbox::SandboxCredentialPrerequisite>,
) -> Router {
    router_with_support(
        store,
        registry,
        database,
        database_client,
        owner,
        can_create,
        desktop::CreationSupport {
            connections,
            adapter: None,
            policy: Some(task_experience_policy()),
            machine_image: None,
        },
    )
    .await
    .0
}

async fn router_with_support(
    store: Arc<MongoSandboxStore>,
    registry: Arc<LiveAgentRegistry>,
    database: &mongodb::Database,
    database_client: &mongodb::Client,
    owner: &str,
    can_create: bool,
    support: desktop::CreationSupport,
) -> (Router, crate::agent_lifecycle::LifecycleDrain) {
    let (router, lifecycle) = fixture_with_support(
        store,
        registry,
        database,
        database_client,
        owner,
        can_create,
        support,
    )
    .await;
    (router, lifecycle.drain_handle())
}

async fn fixture_with_support(
    store: Arc<MongoSandboxStore>,
    registry: Arc<LiveAgentRegistry>,
    database: &mongodb::Database,
    database_client: &mongodb::Client,
    owner: &str,
    can_create: bool,
    support: desktop::CreationSupport,
) -> (Router, Arc<InlineAgentLifecycle>) {
    let profiles = Arc::new(
        MongoProfileStore::open(database_client, database.name())
            .await
            .unwrap(),
    );
    let services = AgentProjectionServices::new(
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        store.clone(),
        profiles.clone(),
        registry,
        Arc::new(UnavailableSandboxResources),
        RuntimeCapabilities::new([RuntimeCapability::Conversation, RuntimeCapability::Usage]),
    )
    .with_resource_access(Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(PersonalAuthority),
    )));
    let sandboxes = SandboxService::new(
        store.clone(),
        profiles.clone(),
        SandboxPolicy::default(),
        support.connections,
    );
    let workspaces = WorkspaceService::new(store.clone(), profiles.clone());
    let hierarchy = layer1_public::rest::HierarchyState::new(
        sandboxes.clone(),
        workspaces.clone(),
        layer2_core::sandbox::HierarchyReadService::new(store.clone()).with_resource_access(
            Arc::new(ResourceAccessService::new(
                store.clone(),
                Arc::new(PersonalAuthority),
            )),
        ),
        Arc::new(UnusedCompatibilityLedger),
    );
    let lifecycle = InlineAgentLifecycle::new(
        sandboxes,
        workspaces,
        services.agents(),
        store.clone(),
        layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
        profiles,
        Arc::new(RetiredImageResolver),
        ProfileCapacityBounds {
            min_cpu_millis: 100,
            max_cpu_millis: 8000,
            min_memory_mib: 128,
            max_memory_mib: 16384,
        },
        if support.adapter.is_some() {
            desktop::capacity()
        } else {
            SandboxCapacity::default()
        },
        support.machine_image,
        Arc::new([] as [String; 0]),
        Arc::new(MongoAgentLifecycleStore::open(database).await.unwrap()),
        None,
        support.adapter.as_ref().map(|adapter| {
            layer2_core::sandbox::desktop_creation::DesktopCreationService::new(
                adapter.clone(),
                desktop::image(),
                Some(desktop::image()),
            )
            .unwrap()
        }),
    );
    let lifecycle = match support.policy {
        Some(policy) => lifecycle.with_task_experience(policy),
        None => lifecycle,
    };
    let mut routes = services
        .with_lifecycle(lifecycle.clone())
        .with_lifecycle_operations(lifecycle.clone())
        .router(Arc::new(UnusedCompatibilityLedger))
        .merge(layer1_public::rest::hierarchy_router(hierarchy));
    if let Some(adapter) = support.adapter {
        routes = routes.merge(layer1_public::rest::agent_filesystem_router(
            layer2_core::sandbox::agent_filesystem::AgentFileService::new(
                Arc::new(ResourceAccessService::new(
                    store,
                    Arc::new(PersonalAuthority),
                )),
                adapter,
            ),
        ));
    }
    (
        routes.layer(middleware::from_fn_with_state(
            (SubjectId::new(owner), can_create),
            authenticate_task_owner,
        )),
        lifecycle,
    )
}

async fn authenticate_task_owner(
    axum::extract::State((owner, can_create)): axum::extract::State<(SubjectId, bool)>,
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
    let mut capabilities = vec![Capability::UseOwnSandboxes];
    if can_create {
        capabilities.push(Capability::CreateSandbox);
    }
    let authorization = Authorization::new(capabilities);
    let mut context = v2_tests::context(&owner);
    context.caller = layer2_core::Caller::new(owner.clone(), authorization.clone());
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(owner));
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "local-task-owner".into(),
        ));
    request.extensions_mut().insert(authorization);
    Ok(next.run(request).await)
}

async fn create(router: &Router, body: &Value, key: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/agents")
                .header("authorization", TEST_BEARER)
                .header("content-type", "application/json")
                .header("idempotency-key", key)
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let value = serde_json::from_slice(&to_bytes(response.into_body(), 1024 * 1024).await.unwrap())
        .unwrap();
    (status, value)
}

async fn isolated_environment() -> (
    mongodb::Client,
    mongodb::Database,
    Harness<MongoSandboxStore>,
) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = client.database(&format!(
        "cma_task_recovery_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(
        MongoSandboxStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    let harness = Harness::with_store_services(
        store,
        Arc::new(EmptyAgentSkillResolver),
        1,
        Some(vec![
            ServiceId::new("ornn-api"),
            ServiceId::new("chrono-llm-public"),
        ]),
    )
    .await;
    (client, database, harness)
}

async fn admitted_identity(service: &Router, receipt: &Value) -> AgentId {
    let path = format!(
        "/api/v1/agent-lifecycle/{}",
        receipt["id"].as_str().unwrap()
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let operation: Value =
                serde_json::from_str(&cross_interface_tests::read(service, &path).await).unwrap();
            assert_ne!(operation["status"], "failed", "{operation}");
            if let Some(identity) = operation["agent_id"].as_str() {
                break AgentId(identity.into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_receipt_survives_lost_response_and_reopened_service() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let database_client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database =
        database_client.database(&format!("cma_task_home_{}", uuid::Uuid::new_v4().simple()));
    let store = Arc::new(
        MongoSandboxStore::open(&database_client, database.name())
            .await
            .unwrap(),
    );
    let harness = Harness::with_store_services(
        store,
        Arc::new(EmptyAgentSkillResolver),
        1,
        Some(vec![
            ServiceId::new("ornn-api"),
            ServiceId::new("chrono-llm-public"),
        ]),
    )
    .await;
    let original_router = router(&harness, &database, &database_client).await;
    let input = json!({"name":"New task", "workspace_id":harness.workspace.id, "initial_message":"  比较三个工具\n保留原文。  "});
    let (status, receipt) = create(&original_router, &input, "home-task-original").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    drop(original_router);
    let restarted = router(&harness, &database, &database_client).await;
    let (status, replay) = create(&restarted, &input, "home-task-original").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{replay}");
    assert_eq!(replay["id"], receipt["id"]);
    let operation_path = format!(
        "/api/v1/agent-lifecycle/{}",
        receipt["id"].as_str().unwrap()
    );
    let agent_id = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let operation: Value = serde_json::from_str(
                &cross_interface_tests::read(&restarted, &operation_path).await,
            )
            .unwrap();
            assert_ne!(operation["status"], "failed", "{operation}");
            if let Some(identity) = operation["agent_id"].as_str() {
                break identity.to_string();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_ne!(agent_id, receipt["id"].as_str().unwrap());
    for _attempt in 0..200 {
        harness
            .worker
            .run_one(&AgentId(agent_id.clone()), SystemTime::now())
            .await
            .unwrap();
        let progress: Value = serde_json::from_str(
            &cross_interface_tests::read(
                &restarted,
                &format!("/api/v1/agents/{agent_id}/transcript"),
            )
            .await,
        )
        .unwrap();
        if progress["turns"]
            .as_array()
            .is_some_and(|turns| !turns.is_empty())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let agent: Value = serde_json::from_str(
        &cross_interface_tests::read(&restarted, &format!("/api/v1/agents/{agent_id}")).await,
    )
    .unwrap();
    assert_eq!(agent["first_message"]["purpose"], "task");
    let transcript: Value = serde_json::from_str(
        &cross_interface_tests::read(&restarted, &format!("/api/v1/agents/{agent_id}/transcript"))
            .await,
    )
    .unwrap();
    let messages: Vec<&Value> = transcript["turns"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|turn| turn["owner_message"] == "  比较三个工具\n保留原文。  ")
        .collect();
    assert_eq!(messages.len(), 1, "{agent}: {transcript}");
    let (_, repeated) = create(&restarted, &input, "home-task-original").await;
    assert_eq!(repeated["id"], receipt["id"]);
    let same_history: Value = serde_json::from_str(
        &cross_interface_tests::read(&restarted, &format!("/api/v1/agents/{agent_id}/transcript"))
            .await,
    )
    .unwrap();
    assert_eq!(same_history, transcript);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_validation_rejects_invalid_text_before_retaining_a_key() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let database_client = mongodb::Client::with_uri_str(uri).await.unwrap();
    let database = database_client.database(&format!(
        "cma_task_validation_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let store = Arc::new(
        MongoSandboxStore::open(&database_client, database.name())
            .await
            .unwrap(),
    );
    let harness = Harness::with_store_services(
        store,
        Arc::new(EmptyAgentSkillResolver),
        1,
        Some(vec![
            ServiceId::new("ornn-api"),
            ServiceId::new("chrono-llm-public"),
        ]),
    )
    .await;
    let service = router(&harness, &database, &database_client).await;
    for text in [" \n ".into(), "你".repeat(87382)] {
        let invalid =
            json!({"name":"New task", "workspace_id":harness.workspace.id, "initial_message":text});
        let (status, problem) = create(&service, &invalid, "validation-key").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
        assert!(
            problem["detail"]
                .as_str()
                .unwrap()
                .contains("initial_message"),
            "{problem}"
        );
    }
    let repository_task = json!({"name":"New task", "workspace_id":harness.workspace.id, "repository":{"url":"https://github.com/example/project"}, "initial_message":"Inspect repository"});
    let (status, problem) = create(&service, &repository_task, "validation-key").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert!(
        problem["detail"]
            .as_str()
            .unwrap()
            .contains("existing Workspace"),
        "{problem}"
    );
    for invalid_context in [
        json!({"sandbox_id":harness.workspace.sandbox_id, "repository":{"url":"https://github.com/example/project"}}),
        json!({"repository":{"url":"https://github.com/example/project", "ref":"bad ref"}}),
        json!({"repository":{"url":"https://github.com/example/project", "ref":"--unsafe"}}),
        json!({"repository":{"url":"https://github.com/example/project", "ref":"main..other"}}),
        json!({"repository":{"url":"https://github.com/example/%2e%2e"}}),
    ] {
        let mut invalid = json!({"name":"New task", "initial_message":"Inspect repository"});
        invalid
            .as_object_mut()
            .unwrap()
            .extend(invalid_context.as_object().unwrap().clone());
        let (status, problem) = create(&service, &invalid, "validation-key").await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    }
    let valid = json!({"name":"New task", "workspace_id":harness.workspace.id, "initial_message":"A valid task"});
    let (status, receipt) = create(&service, &valid, "validation-key").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    database.drop().await.unwrap();
}
