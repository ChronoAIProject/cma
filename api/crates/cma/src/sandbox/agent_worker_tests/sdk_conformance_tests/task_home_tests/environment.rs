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

pub(super) struct LocalIdentity;

use super::repository_connection_tests::{RepositoryConnection, RepositoryRefusal};

#[async_trait::async_trait]
impl OwnerTokenSource for LocalIdentity {
    async fn token_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, String> {
        Ok(Some(SecretValue::new("local-task-owner".into())))
    }
}

pub(super) async fn identity_provider() -> (String, tokio::task::JoinHandle<()>) {
    let routes = Router::new()
        .merge(super::integrated_tests::catalog::identity_routes())
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
    if std::env::var_os("CMA_INTEGRATED_RELEASE_PROOF").is_some() {
        super::integrated_tests::run().await;
        return;
    }
    initial_task_environment(None, None, false).await;
    initial_task_environment(None, None, true).await;
}

pub(super) async fn initial_task_environment(
    repository: Option<Value>,
    source_failure: Option<RepositoryRefusal>,
    desktop: bool,
) {
    let retry_unavailable = store_retry::StoreRetry::new(desktop);
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let marker = source_failure
        .filter(|failure| failure.before_marker)
        .map(|failure| super::repository_marker::MarkerPause::new(failure.delay_initial_renewal));
    let database_client = match &marker {
        Some(marker) => marker.client(&uri).await,
        None => mongodb::Client::with_uri_str(&uri).await.unwrap(),
    };
    let database = database_client.database(&format!(
        "cma_task_default_{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut key_bytes = [0_u8; 32];
    SystemRandom::new().fill(&mut key_bytes).unwrap();
    let encryption = AuthEncryptionKey::from_base64(&STANDARD.encode(key_bytes)).unwrap();
    let version = if desktop {
        super::desktop::image()
    } else {
        CodexSandboxSpec::default().deployment.image.uri
    };
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
    let store = Arc::new((*stores.sandbox_model).clone().with_renewable_work(true));
    let mut harness = Harness::with_store_services(
        store,
        Arc::new(EmptyAgentSkillResolver),
        1,
        Some(vec![
            ServiceId::new("ornn-api"),
            ServiceId::new("chrono-llm-public"),
        ]),
    )
    .await;
    let desktop_runtime = if desktop {
        Some(super::desktop::configure(&mut harness).await)
    } else {
        None
    };
    let connection = RepositoryConnection::new();
    let (service, lifecycle) = fixture_with_support(
        harness.store.clone(),
        harness.registry.clone(),
        &database,
        &database_client,
        "owner",
        true,
        super::desktop::CreationSupport {
            connections: connection.prerequisite(),
            adapter: desktop.then(|| harness.adapter.clone()),
            policy: Some(task_experience_policy()),
            machine_image: None,
        },
    )
    .await;
    let lifecycle_drain = lifecycle.drain_handle();
    lifecycle.resume_pending();
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
    let mut workspace_config =
        WorkspaceWorkerConfig::new("task-default-workspace", DEPLOYMENT_ID).with_replicas(true);
    workspace_config.retry_delay = Duration::from_secs(1);
    let workspace_worker = WorkspaceWorker::new(
        WorkspaceWorkerDependencies {
            store: harness.store.clone(),
            adapter: harness.adapter.clone(),
        },
        workspace_config,
    );
    let mut sandbox_config = SandboxWorkerConfig::new(
        "task-default-sandbox",
        DEPLOYMENT_ID,
        version,
        SandboxPolicy::default(),
    )
    .with_replicas(true)
    .with_home_policy(desktop.then(super::desktop::home_policy));
    sandbox_config.retry_delay = Duration::from_secs(1);
    let worker = |config| {
        SandboxWorker::new(
            SandboxWorkerDependencies {
                store: harness.store.clone(),
                adapter: harness.adapter.clone(),
                credentials: credentials.clone(),
                workspaces: Arc::new(DurableProfileWorkspaceProvisioner::new(
                    WorkspaceService::new(harness.store.clone(), profiles.clone()),
                    workspace_worker.clone(),
                )),
                agents: Arc::new(DurableLiveAgentStopper::new(
                    harness.store.clone(),
                    harness.registry.clone(),
                )),
                registry: harness.registry.clone(),
                updates: harness.updates.clone(),
                operations: AgentOperationService::new(
                    harness.store.clone(),
                    harness.registry.clone(),
                ),
            },
            config,
        )
    };
    let sandbox_worker = worker(sandbox_config.clone());
    let original = "  比较三个工具\n保留原文。  ";
    let mut input = json!({"name":"New task", "initial_message":original});
    if desktop {
        input["desktop"] = json!(true);
    }
    if let Some(repository) = &repository {
        input["repository"] = repository.clone();
    }
    let public_server = if desktop {
        super::desktop_native::serve(service.clone(), &input).await
    } else {
        None
    };
    let (creation_key, receipt) = if public_server.is_some() {
        super::desktop_native::creation(&input).await
    } else {
        let key = if desktop {
            "process-restart-task"
        } else {
            "default-environment-task"
        };
        let (status, receipt) = create(&service, &input, key).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
        (key.to_owned(), receipt)
    };
    let operation_path = format!(
        "/api/v1/agent-lifecycle/{}",
        receipt["id"].as_str().unwrap()
    );
    if repository.is_some() {
        connection
            .allow_after_blocked_states(&service, &harness, &input, &receipt, &operation_path)
            .await;
    }
    if let Some(marker) = marker {
        let operation_id = receipt["id"].as_str().unwrap();
        let refusal = source_failure.unwrap();
        let sandbox = marker
            .fail_before_marker(
                &database,
                operation_id,
                &harness,
                &sandbox_worker,
                refusal.code,
                &store_retry::StoreRetry::new(desktop),
            )
            .await;
        sandbox_config.holder = "task-cleanup-before-marker-restarted".into();
        let resumed = worker(sandbox_config);
        super::repository_connection_tests::complete_failed_environment_retirement(
            &harness,
            &sandbox,
            None,
            &resumed,
            &retry_unavailable,
        )
        .await;
        provider_task.abort();
        lifecycle_drain.drain(Duration::from_secs(5)).await.unwrap();
        database.drop().await.unwrap();
        return;
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
    let transcript_path = format!("{agent_path}/transcript");
    if let Some(refusal) = source_failure {
        let code = refusal.code;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        let failed_agent = super::environment_workers::drive_source_failure(
            &harness,
            &agent_id,
            &sandbox_id,
            &sandbox_worker,
            &retry_unavailable,
            refusal,
            deadline,
        )
        .await;
        // Workspace failure publishes the operation and its teardown obligation together.
        let operation: Value = tokio::time::timeout_at(deadline, async {
            let mut tick = tokio::time::interval(Duration::from_millis(20));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tick.tick().await;
                let operation: Value = serde_json::from_str(
                    &cross_interface_tests::read(&service, &operation_path).await,
                )
                .unwrap();
                match operation["status"].as_str() {
                    Some("failed") => break operation,
                    Some("running" | "pending") => {}
                    status => panic!("source case {code}: unexpected public status {status:?}"),
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("source case {code}: terminal failure was not projected"));
        assert!(
            operation["error"]
                .as_str()
                .is_some_and(|detail| detail.contains(code)),
            "public operation lost the source failure code: {code}"
        );
        assert!(!operation.to_string().contains("External failure detail"));
        let transcript: Value =
            serde_json::from_str(&cross_interface_tests::read(&service, &transcript_path).await)
                .unwrap();
        assert_eq!(transcript["turns"], json!([]));
        let first = failed_agent.conversation.first_message.as_ref().unwrap();
        assert_eq!(first.text, original);
        assert!(first.dispatch_fenced_at.is_none() && first.turn_sequence.is_none());
        assert!(first.write_confirmed_at.is_none() && first.execution_observed_at.is_none());
        assert!(!harness
            .backend
            .requests()
            .await
            .iter()
            .any(|(_, request)| matches!(request,
            layer3_adapter::wire::RequestFrame::OpenAgent(frame)
                if frame.request.agent_id.as_str() == agent_id.as_str())));
        let (status, replay) = create(&service, &input, "default-environment-task").await;
        assert_eq!(status, StatusCode::ACCEPTED, "{replay}");
        assert_eq!(replay["id"], receipt["id"]);
        let operation: Value =
            serde_json::from_str(&cross_interface_tests::read(&service, &operation_path).await)
                .unwrap();
        assert_eq!(operation["status"], "failed");
        assert_eq!(operation["agent_id"], agent_id.as_str());
        let retained = harness.stored(&agent_id).await;
        assert_eq!(retained.workspace_id, failed_agent.workspace_id);
        assert_eq!(retained.worktree_id, failed_agent.worktree_id);
        assert_eq!(retained.conversation.first_message.as_ref(), Some(first));
        if refusal.sibling {
            super::repository_connection_tests::assert_other_workspace_preserved(
                &harness,
                &failed_agent,
            )
            .await;
        } else {
            sandbox_config.holder = "task-cleanup-restarted".into();
            let resumed = worker(sandbox_config);
            super::repository_connection_tests::complete_failed_environment_retirement(
                &harness,
                &failed_agent.sandbox_id,
                Some(&failed_agent),
                &resumed,
                &retry_unavailable,
            )
            .await;
        }
        provider_task.abort();
        lifecycle_drain.drain(Duration::from_secs(5)).await.unwrap();
        database.drop().await.unwrap();
        return;
    }
    let mut phase = String::from("reading admitted Sandbox");
    let completion = tokio::time::timeout(Duration::from_secs(60), async {
        super::environment_workers::drive_environment(
            &harness,
            &agent_id,
            &sandbox_id,
            &sandbox_worker,
            &retry_unavailable,
            None,
            &mut phase,
        )
        .await;
        phase = String::from("waiting for the first task to be delivered");
        loop {
            if retry_unavailable.agent(&harness, &agent_id).await.is_none() {
                tokio::time::sleep(Duration::from_millis(20)).await;
                continue;
            }
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
    assert!(
        completion.is_ok(),
        "timed out at {phase}: {}",
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
    let (_, replay) = create(&service, &input, &creation_key).await;
    assert_eq!(replay["id"], receipt["id"]);
    if desktop {
        let restarted = router_for(&harness, &database, &database_client, "owner", true).await;
        let (status, recovered) = create(&restarted, &input, &creation_key).await;
        assert_eq!(status, StatusCode::ACCEPTED, "{recovered}");
        assert_eq!(recovered["id"], receipt["id"]);
        let mut changed = input.clone();
        changed["desktop"] = json!(false);
        let (status, _) = create(&restarted, &changed, &creation_key).await;
        assert_eq!(status, StatusCode::CONFLICT);
        desktop_runtime.as_ref().unwrap().verify_recovery();
        super::desktop::verify_preflight(&harness, &agent_id).await;
        if public_server.is_none() {
            super::desktop_native::verify_process_recovery(
                &database,
                &input,
                &receipt,
                &harness.stored(&agent_id).await,
            )
            .await;
            super::desktop::verify_existing_admission(&service, &harness, &agent_id).await;
        }
    }
    if public_server.is_some() {
        super::desktop_native::finish().await;
    }
    if let Some(server) = public_server {
        server.abort();
    }
    provider_task.abort();
    lifecycle_drain.drain(Duration::from_secs(5)).await.unwrap();
    database.drop().await.unwrap();
}
