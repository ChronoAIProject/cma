use super::*;
use layer2_core::sandbox::task_experience::TaskExperiencePolicy;
use layer3_adapter::hosted::HostedBackend;

mod replay_tests;

fn policy(account: &str, capabilities: &[&str]) -> Arc<TaskExperiencePolicy> {
    Arc::new(
        serde_json::from_value(json!({
            "mode": "accounts", "accounts": [account], "capabilities": capabilities
        }))
        .unwrap(),
    )
}

fn machine_image(digest_digit: &str) -> String {
    format!("runtime.example/desktop@sha256:{}", digest_digit.repeat(64))
}

async fn configured_router(
    harness: &Harness<MongoSandboxStore>,
    database: &mongodb::Database,
    client: &mongodb::Client,
    policy: Option<Arc<TaskExperiencePolicy>>,
    can_create: bool,
) -> (Router, crate::agent_lifecycle::LifecycleDrain) {
    configured_router_for_image(
        harness,
        database,
        client,
        policy,
        can_create,
        machine_image("a"),
    )
    .await
}

async fn configured_router_for_image(
    harness: &Harness<MongoSandboxStore>,
    database: &mongodb::Database,
    client: &mongodb::Client,
    policy: Option<Arc<TaskExperiencePolicy>>,
    can_create: bool,
    image: String,
) -> (Router, crate::agent_lifecycle::LifecycleDrain) {
    router_with_support(
        harness.store.clone(),
        harness.registry.clone(),
        database,
        client,
        "owner",
        can_create,
        desktop::CreationSupport {
            connections: Arc::new(
                layer2_core::sandbox::FixedSandboxCredentialPrerequisite::SetupRequired,
            ),
            adapter: None,
            policy,
            machine_image: Some(image),
        },
    )
    .await
}

async fn assert_refused_without_admission(
    service: &Router,
    database: &mongodb::Database,
    harness: &Harness<MongoSandboxStore>,
    body: Value,
    expected: StatusCode,
) {
    let before = recovery::resource_counts(database).await;
    let runtimes = harness.backend.inventory().await.unwrap();
    let key = uuid::Uuid::new_v4().to_string();
    let (status, problem) = create(service, &body, &key).await;
    assert_eq!(status, expected, "{problem}");
    assert_eq!(recovery::resource_counts(database).await, before);
    assert_eq!(harness.backend.inventory().await.unwrap(), runtimes);
    let ledger = MongoAgentLifecycleStore::open(database).await.unwrap();
    assert!(ledger
        .replay(
            &SubjectId::new("owner"),
            &key,
            &serde_json::from_value(body).unwrap(),
        )
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn machine_only_requires_current_account_desktop_policy_before_admission() {
    let (client, database, harness) = isolated_environment().await;
    for configured in [
        Some(policy("excluded-owner", &["desktop_enable"])),
        None,
        Some(Arc::new(TaskExperiencePolicy::default())),
        Some(policy("owner", &["task_home_admission"])),
    ] {
        let (service, drain) =
            configured_router(&harness, &database, &client, configured, true).await;
        assert_refused_without_admission(
            &service,
            &database,
            &harness,
            json!({"name":"Machine", "machine_included":true}),
            StatusCode::FORBIDDEN,
        )
        .await;
        drain.drain(Duration::from_secs(5)).await.unwrap();
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn machine_only_admits_enabled_account_without_task_home_capability() {
    let (client, database, harness) = isolated_environment().await;
    let (service, drain) = configured_router(
        &harness,
        &database,
        &client,
        Some(policy("owner", &["desktop_enable"])),
        true,
    )
    .await;
    let (status, receipt) = create(
        &service,
        &json!({"name":"Machine", "machine_included":true}),
        "enabled-machine",
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    drain.drain(Duration::from_secs(5)).await.unwrap();
    let record = MongoAgentLifecycleStore::open(&database)
        .await
        .unwrap()
        .read_for_owner(&SubjectId::new("owner"), receipt["id"].as_str().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(record.input.request.machine_included);
    assert!(record.input.request.initial_message.is_none());
    assert_eq!(record.input.resolved_image, Some(machine_image("a")));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn desktop_policy_preserves_message_and_ordinary_create_boundaries() {
    let (client, database, harness) = isolated_environment().await;
    for (capabilities, machine, message, expected) in [
        (vec!["desktop_enable"], true, true, StatusCode::FORBIDDEN),
        (vec!["desktop_enable"], false, true, StatusCode::FORBIDDEN),
        (
            vec!["task_home_admission"],
            false,
            true,
            StatusCode::ACCEPTED,
        ),
        (
            vec!["task_home_admission", "desktop_enable"],
            true,
            true,
            StatusCode::ACCEPTED,
        ),
        (vec![], false, false, StatusCode::ACCEPTED),
    ] {
        let configured = if capabilities.is_empty() {
            Some(Arc::new(TaskExperiencePolicy::default()))
        } else {
            Some(policy("owner", &capabilities))
        };
        let (service, drain) =
            configured_router(&harness, &database, &client, configured, true).await;
        let mut body = json!({"name":"Create", "machine_included":machine});
        if message {
            body["initial_message"] = json!("Inspect the workspace");
        }
        if expected == StatusCode::FORBIDDEN {
            assert_refused_without_admission(&service, &database, &harness, body, expected).await;
        } else {
            let (status, receipt) =
                create(&service, &body, &uuid::Uuid::new_v4().to_string()).await;
            assert_eq!(status, expected, "{receipt}");
        }
        drain.drain(Duration::from_secs(5)).await.unwrap();
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn desktop_policy_does_not_replace_validation_or_resource_authority() {
    let (client, database, harness) = isolated_environment().await;
    for (can_create, body, expected) in [
        (
            false,
            json!({"name":"Machine", "machine_included":true}),
            StatusCode::FORBIDDEN,
        ),
        (
            true,
            json!({"name":"Desktop", "desktop":true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            true,
            json!({"name":"Desktop", "desktop":true, "machine_included":true}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (service, drain) = configured_router(
            &harness,
            &database,
            &client,
            Some(task_experience_policy()),
            can_create,
        )
        .await;
        assert_refused_without_admission(&service, &database, &harness, body, expected).await;
        drain.drain(Duration::from_secs(5)).await.unwrap();
    }
    let (service, drain) = configured_router(
        &harness,
        &database,
        &client,
        Some(policy("owner", &["task_home_admission"])),
        true,
    )
    .await;
    assert_refused_without_admission(
        &service,
        &database,
        &harness,
        json!({"name":"Desktop", "desktop":true, "initial_message":"Open the page"}),
        StatusCode::FORBIDDEN,
    )
    .await;
    drain.drain(Duration::from_secs(5)).await.unwrap();
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn verified_desktop_default_can_share_the_canary_manifest_digest() {
    use crate::store::{CreateResult, LifecycleBehavior, LifecycleInput};

    let (client, database, mut harness) = isolated_environment().await;
    desktop::configure(&mut harness).await;
    let operations = MongoAgentLifecycleStore::open(&database).await.unwrap();
    let owner = SubjectId::new("owner");
    let input = LifecycleInput {
        behavior: LifecycleBehavior::Legacy,
        managed_agent_instructions: None,
        setup_message: None,
        request: serde_json::from_value(json!({"name": "Runtime canary fixture"})).unwrap(),
        resolved_image: None,
        capacity: Some(desktop::capacity()),
        sandbox_availability: Default::default(),
        capabilities: vec![],
        actor: None,
    };
    let CreateResult::Created(id) = operations
        .create(&owner, "canary-fixture", input)
        .await
        .unwrap()
    else {
        panic!("expected a new canary fixture");
    };
    let mut canary = operations.claim(&id).await.unwrap().unwrap();
    let image = desktop::image();
    let canary_profiles = operations
        .install_canary_worker_fixture(
            &mut canary,
            layer3_adapter::RuntimeImage {
                uri: image.clone(),
                ..Default::default()
            },
        )
        .await;

    let (router, drain) = router_with_support(
        harness.store.clone(),
        harness.registry.clone(),
        &database,
        &client,
        owner.as_str(),
        true,
        desktop::CreationSupport {
            connections: Arc::new(layer2_core::sandbox::FixedSandboxCredentialPrerequisite::Ready),
            adapter: Some(harness.adapter.clone()),
            policy: Some(task_experience_policy()),
            machine_image: None,
        },
    )
    .await;
    let (status, receipt) = create(
        &router,
        &json!({"name": "Ordinary desktop", "desktop": true, "initial_message": "Open the page"}),
        "ordinary-desktop",
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let desktop_id = receipt["id"].as_str().unwrap();
    let (record, sandbox_id) = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let record = operations
                .read_for_owner(&owner, desktop_id)
                .await
                .unwrap()
                .unwrap();
            assert_ne!(record.status, "failed", "{:?}", record.error);
            if let Some(sandbox_id) = record.sandbox_id.clone() {
                break (record, layer2_core::SandboxId(sandbox_id));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(record.input.resolved_image.as_deref(), Some(image.as_str()));
    assert!(record.input.request.desktop);
    assert!(!record.input.request.machine_included);
    let sandbox = harness
        .store
        .get_sandbox(&sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox.owner, owner);
    assert_eq!(
        sandbox.origin,
        layer2_core::sandbox::SandboxOrigin::Lifecycle {
            operation_id: desktop_id.into()
        },
    );
    assert_ne!(sandbox.profile.id, canary_profiles.sandbox.id);
    assert_eq!(
        sandbox.profile.content.runtime_image.as_deref(),
        Some(image.as_str())
    );
    drain.drain(Duration::from_secs(5)).await.unwrap();
    database.drop().await.unwrap();
}
