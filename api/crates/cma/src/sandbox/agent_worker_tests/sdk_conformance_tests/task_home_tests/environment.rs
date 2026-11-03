use super::*;
use crate::auth::{keys::KeyIssuer, persistence::SecretValue};
use crate::sandbox::credential::{ManagedSandboxCredentials, OwnerTokenSource};
use crate::sandbox::live_agent_stopper::DurableLiveAgentStopper;
use crate::sandbox::worker::{SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies};
use crate::sandbox::workspace_worker::{
    DurableProfileWorkspaceProvisioner, WorkspaceWorker, WorkspaceWorkerConfig,
    WorkspaceWorkerDependencies,
};
use crate::store::{mongo::MongoStores, AuthEncryptionKey};
use axum::{
    routing::{delete, get, post},
    Json,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ring::rand::{SecureRandom, SystemRandom};

struct LocalIdentity;

struct RepositoryConnection(std::sync::atomic::AtomicBool);

#[async_trait::async_trait]
impl layer2_core::sandbox::SandboxCredentialPrerequisite for RepositoryConnection {
    async fn check(
        &self,
        _: &SubjectId,
    ) -> Result<(), layer2_core::sandbox::SandboxCredentialPrerequisiteError> {
        Ok(())
    }

    async fn check_services(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<(), layer2_core::sandbox::SandboxCredentialPrerequisiteError> {
        if services
            .iter()
            .any(|service| service.as_str() == "api-github")
            && !self.0.load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(
                layer2_core::sandbox::SandboxCredentialPrerequisiteError::ConnectionsRequired {
                    services: vec![ServiceId::new("api-github")],
                },
            )
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl OwnerTokenSource for LocalIdentity {
    async fn token_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, String> {
        Ok(Some(SecretValue::new("local-task-owner".into())))
    }
}

async fn identity_provider() -> (String, tokio::task::JoinHandle<()>) {
    let routes = Router::new()
        .route(
            "/api/v1/user-services",
            get(|| async {
                Json(json!([
                    {"id":"provider-model", "slug":"chrono-llm-public", "is_active":true},
                    {"id":"provider-skills", "slug":"ornn-api", "is_active":true},
                    {"id":"provider-github", "slug":"api-github", "is_active":true}
                ]))
            }),
        )
        .route(
            "/api/v1/api-keys/scope-plan",
            post(|Json(body): Json<Value>| async move {
                Json(crate::auth::keys::scope_plan_fixture("owner", &body, &[]))
            }),
        )
        .route(
            "/api/v1/api-keys",
            post(|Json(body): Json<Value>| async move {
                Json(json!({
                    "id":"local-task-key", "full_key":"local-task-runtime-credential",
                    "scopes":"proxy read", "state_version":1,
                    "allowed_service_ids":body["allowed_service_ids"],
                    "allowed_node_ids":body["allowed_node_ids"],
                    "allow_all_services":false, "allow_all_nodes":false
                }))
            }),
        )
        .route(
            "/api/v1/api-keys/{id}",
            delete(|| async { StatusCode::NO_CONTENT }),
        )
        .route("/proxy", get(|| async { StatusCode::NO_CONTENT }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, routes).await.unwrap();
    });
    (endpoint, task)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_default_environment_delivers_exact_task_after_preparation() {
    initial_task_environment(None, None).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_task_public_preparation_pins_checkout_before_exact_task_and_replays_identity() {
    initial_task_environment(
        Some(json!({"url":"https://github.com/example/project", "ref":"proof/fixed"})),
        None,
    )
    .await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn repository_task_public_source_failure_preserves_identity_without_dispatching_task() {
    for failure in [
        "repository_ref_not_found",
        "repository_unavailable",
        "repository_source_invalid",
    ] {
        initial_task_environment(
            Some(json!({"url":"https://github.com/example/project", "ref":"proof/fixed"})),
            Some(failure),
        )
        .await;
    }
}

async fn initial_task_environment(repository: Option<Value>, source_failure: Option<&str>) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let database_client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let database = database_client.database(&format!(
        "cma_task_default_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut key_bytes = [0_u8; 32];
    SystemRandom::new().fill(&mut key_bytes).unwrap();
    let encryption = AuthEncryptionKey::from_base64(&STANDARD.encode(key_bytes)).unwrap();
    let version = CodexSandboxSpec::default().deployment.image.uri;
    database
        .run_command(crate::store::isolation::role(database.name()))
        .await
        .unwrap();
    let password = uuid::Uuid::new_v4().to_string();
    database
        .run_command(mongodb::bson::doc! {
            "createUser":"task-runtime", "pwd":&password,
            "roles":[{"role":"cma_am_runtime", "db":database.name()}]
        })
        .await
        .unwrap();
    let mut runtime_uri = reqwest::Url::parse(&uri).unwrap();
    runtime_uri.set_username("task-runtime").unwrap();
    runtime_uri.set_password(Some(&password)).unwrap();
    runtime_uri
        .query_pairs_mut()
        .append_pair("authSource", database.name());
    let stores = MongoStores::connect(
        runtime_uri.as_str(),
        database.name(),
        DEPLOYMENT_ID,
        &encryption,
        Duration::from_secs(3600),
        (&version, None),
    )
    .await
    .unwrap();
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
    let connection = Arc::new(RepositoryConnection(std::sync::atomic::AtomicBool::new(
        false,
    )));
    let service = router_with_connections(
        harness.store.clone(),
        harness.registry.clone(),
        &database,
        &database_client,
        "owner",
        true,
        connection.clone(),
    )
    .await;
    let (provider, provider_task) = identity_provider().await;
    let credentials = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(
            reqwest::Client::new(),
            provider,
            DEPLOYMENT_ID,
        )),
        Arc::new(LocalIdentity),
        stores.sandbox_credentials.clone(),
    );
    let profiles = Arc::new(
        MongoProfileStore::open(&database_client, database.name())
            .await
            .unwrap(),
    );
    let mut workspace_config = WorkspaceWorkerConfig::new("task-default-workspace", DEPLOYMENT_ID);
    workspace_config.retry_delay = Duration::from_secs(1);
    let workspace_worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
        },
        workspace_config,
    );
    let sandbox_worker = SandboxWorker::new(
        SandboxWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
            credentials,
            workspaces: Arc::new(DurableProfileWorkspaceProvisioner::new(
                WorkspaceService::new(harness.store.clone(), profiles),
                workspace_worker.clone(),
            )),
            agents: Arc::new(DurableLiveAgentStopper::new(
                harness.store.clone(),
                harness.registry.clone(),
            )),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        SandboxWorkerConfig::new(
            "task-default-sandbox",
            DEPLOYMENT_ID,
            version,
            SandboxPolicy::default(),
        ),
    );
    let original = "  比较三个工具\n保留原文。  ";
    let mut input = json!({"name":"New task", "initial_message":original});
    if let Some(repository) = &repository {
        input["repository"] = repository.clone();
    }
    let (status, receipt) = create(&service, &input, "default-environment-task").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let operation_path = format!(
        "/api/v1/agent-lifecycle/{}",
        receipt["id"].as_str().unwrap()
    );
    if repository.is_some() {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let operation: Value = serde_json::from_str(
                    &cross_interface_tests::read(&service, &operation_path).await,
                )
                .unwrap();
                if operation["status"] == "waiting_for_connection" {
                    assert!(operation["agent_id"].is_null());
                    assert_eq!(operation["connection"]["services"], json!(["api-github"]));
                    break;
                }
                assert_ne!(operation["status"], "failed", "{operation}");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let (_, waiting_replay) = create(&service, &input, "default-environment-task").await;
        assert_eq!(waiting_replay["id"], receipt["id"]);
        connection
            .0
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let (_, authorized) = create(&service, &input, "default-environment-task").await;
        assert_eq!(authorized["id"], receipt["id"]);
    }
    let agent_id = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let operation: Value =
                serde_json::from_str(&cross_interface_tests::read(&service, &operation_path).await)
                    .unwrap();
            assert_ne!(operation["status"], "failed", "{operation}");
            if let Some(identity) = operation["agent_id"].as_str() {
                break AgentId(identity.into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let agent_path = format!("/api/v1/agents/{agent_id}");
    let admitted: Value =
        serde_json::from_str(&cross_interface_tests::read(&service, &agent_path).await).unwrap();
    assert_eq!(admitted["first_message"]["purpose"], "task");
    let sandbox_id = SandboxId(admitted["sandbox"]["id"].as_str().unwrap().into());
    assert_ne!(sandbox_id, harness.workspace.sandbox_id);
    if let Some(code) = source_failure {
        harness.backend.fail_next(
            WireOperation::PrepareWorkspace,
            code,
            "External failure detail must not leak",
        );
    }
    let environment_driver = tokio::spawn(async move {
        loop {
            sandbox_worker
                .run_one(&sandbox_id, SystemTime::now())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let workspace_driver = tokio::spawn(async move {
        loop {
            workspace_worker.run_due(SystemTime::now()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    let transcript_path = format!("{agent_path}/transcript");
    if let Some(code) = source_failure {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                harness
                    .worker
                    .run_one(&agent_id, SystemTime::now())
                    .await
                    .unwrap();
                let operation: Value = serde_json::from_str(
                    &cross_interface_tests::read(&service, &operation_path).await,
                )
                .unwrap();
                if operation["status"] == "failed" {
                    assert!(
                        operation["error"]
                            .as_str()
                            .is_some_and(|detail| detail.contains(code)),
                        "{operation}"
                    );
                    assert!(!operation.to_string().contains("External failure detail"));
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let transcript: Value =
            serde_json::from_str(&cross_interface_tests::read(&service, &transcript_path).await)
                .unwrap();
        assert_eq!(transcript["turns"], json!([]));
        let (_, replay) = create(&service, &input, "default-environment-task").await;
        assert_eq!(replay["id"], receipt["id"]);
        environment_driver.abort();
        workspace_driver.abort();
        provider_task.abort();
        database.drop().await.unwrap();
        return;
    }
    let completion = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            harness
                .worker
                .run_one(&agent_id, SystemTime::now())
                .await
                .unwrap();
            let transcript: Value = serde_json::from_str(
                &cross_interface_tests::read(&service, &transcript_path).await,
            )
            .unwrap();
            if let Some(turns) = transcript["turns"]
                .as_array()
                .filter(|turns| !turns.is_empty())
            {
                assert_eq!(turns.len(), 1, "{transcript}");
                assert_eq!(turns[0]["owner_message"], original);
                break;
            }
            let progress: Value =
                serde_json::from_str(&cross_interface_tests::read(&service, &agent_path).await)
                    .unwrap();
            assert_ne!(progress["status"], "failed", "{progress}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    environment_driver.abort();
    workspace_driver.abort();
    assert!(
        completion.is_ok(),
        "{}",
        cross_interface_tests::read(&service, &agent_path).await
    );
    if let Some(repository) = repository {
        let detail: Value =
            serde_json::from_str(&cross_interface_tests::read(&service, &agent_path).await)
                .unwrap();
        let workspace = &detail["workspace"];
        assert_eq!(workspace["source"]["url"], repository["url"]);
        assert_eq!(workspace["source"]["reference"], repository["ref"]);
        assert!(workspace["resolved_commit"]
            .as_str()
            .is_some_and(|commit| commit.len() >= 40));
        assert_eq!(
            detail["workspace"]["resolved_commit"],
            workspace["resolved_commit"]
        );
        assert_eq!(
            detail["worktree"]["base_commit"],
            workspace["resolved_commit"]
        );
        assert_eq!(workspace["source"]["source_commit"], "a".repeat(40));
    }
    let (_, replay) = create(&service, &input, "default-environment-task").await;
    assert_eq!(replay["id"], receipt["id"]);
    provider_task.abort();
    database.drop().await.unwrap();
}
