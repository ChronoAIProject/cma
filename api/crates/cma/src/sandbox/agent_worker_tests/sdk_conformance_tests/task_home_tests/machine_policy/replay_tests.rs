use super::*;
use crate::store::LifecycleRecord;

async fn waiting_record(database: &mongodb::Database, id: &str, attempt: u32) -> LifecycleRecord {
    let store = MongoAgentLifecycleStore::open(database).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let record = store
                .read_for_owner(&SubjectId::new("owner"), id)
                .await
                .unwrap()
                .unwrap();
            assert_ne!(record.status, "failed", "{record:?}");
            if record.status == "waiting_for_connection" && record.attempts > attempt {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

async fn retain_machine(
    harness: &Harness<MongoSandboxStore>,
    database: &mongodb::Database,
    client: &mongodb::Client,
    body: &Value,
    key: &str,
) -> LifecycleRecord {
    let (service, drain) = configured_router(
        harness,
        database,
        client,
        Some(task_experience_policy()),
        true,
    )
    .await;
    let (status, receipt) = create(&service, body, key).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    // A real connection wait makes accidental resume or worker reclaim observable
    // in the durable status, attempt count and event history after reopening.
    let record = waiting_record(database, receipt["id"].as_str().unwrap(), 0).await;
    drain.drain(Duration::from_secs(5)).await.unwrap();
    record
}

async fn assert_retained_refusal(
    service: &Router,
    drain: crate::agent_lifecycle::LifecycleDrain,
    database: &mongodb::Database,
    harness: &Harness<MongoSandboxStore>,
    retained: &LifecycleRecord,
    body: &Value,
    expected: StatusCode,
) {
    let counts = recovery::resource_counts(database).await;
    let runtimes = harness.backend.inventory().await.unwrap();
    let (status, problem) = create(service, body, &retained.key).await;
    // Join any accidentally started worker before comparing retained state.
    drain.drain(Duration::from_secs(5)).await.unwrap();
    assert_eq!(status, expected, "{problem}");
    let after = MongoAgentLifecycleStore::open(database)
        .await
        .unwrap()
        .read_for_owner(&SubjectId::new("owner"), &retained.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(after).unwrap(),
        serde_json::to_value(retained).unwrap()
    );
    assert_eq!(recovery::resource_counts(database).await, counts);
    assert_eq!(harness.backend.inventory().await.unwrap(), runtimes);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn retained_machine_rechecks_current_policy_before_resuming() {
    let (client, database, harness) = isolated_environment().await;
    let body = json!({"name":"Retained machine", "machine_included":true});
    let retained = retain_machine(&harness, &database, &client, &body, "retained-machine").await;
    for configured in [
        None,
        Some(Arc::new(TaskExperiencePolicy::default())),
        Some(policy("former-owner", &["desktop_enable"])),
        Some(policy("owner", &["task_home_admission"])),
    ] {
        let (service, drain) =
            configured_router(&harness, &database, &client, configured, true).await;
        assert_retained_refusal(
            &service,
            drain,
            &database,
            &harness,
            &retained,
            &body,
            StatusCode::FORBIDDEN,
        )
        .await;
    }
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn eligible_machine_replay_preserves_identity_and_request_authority() {
    let (client, database, harness) = isolated_environment().await;
    let body = json!({"name":"Retained machine", "machine_included":true});
    let retained = retain_machine(&harness, &database, &client, &body, "enabled-replay").await;
    for (can_create, request, expected) in [
        (false, body.clone(), StatusCode::FORBIDDEN),
        (
            true,
            json!({"name":"Changed request", "machine_included":true}),
            StatusCode::CONFLICT,
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
        assert_retained_refusal(
            &service, drain, &database, &harness, &retained, &request, expected,
        )
        .await;
    }
    let (foreign, drain) = router_with_support(
        harness.store.clone(),
        harness.registry.clone(),
        &database,
        &client,
        "other-owner",
        true,
        desktop::CreationSupport {
            connections: Arc::new(
                layer2_core::sandbox::FixedSandboxCredentialPrerequisite::SetupRequired,
            ),
            adapter: None,
            policy: Some(task_experience_policy()),
            machine_image: None,
        },
    )
    .await;
    assert_retained_refusal(
        &foreign,
        drain,
        &database,
        &harness,
        &retained,
        &body,
        StatusCode::FORBIDDEN,
    )
    .await;
    let (service, drain) = configured_router(
        &harness,
        &database,
        &client,
        Some(policy("owner", &["desktop_enable"])),
        true,
    )
    .await;
    let counts = recovery::resource_counts(&database).await;
    let (status, receipt) = create(&service, &body, &retained.key).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    assert_eq!(receipt["id"], retained.id);
    let resumed = waiting_record(&database, &retained.id, retained.attempts).await;
    drain.drain(Duration::from_secs(5)).await.unwrap();
    assert_eq!(
        serde_json::to_value(resumed.input).unwrap(),
        serde_json::to_value(retained.input).unwrap()
    );
    assert_eq!(recovery::resource_counts(&database).await, counts);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn machine_replay_retains_the_admitted_digest_after_the_default_changes() {
    let (client, database, mut harness) = isolated_environment().await;
    let body = json!({"name":"Retained machine", "machine_included":true});
    let owner = SubjectId::new("owner");
    let image_a = machine_image("a");
    let image_b = machine_image("b");
    let retained = retain_machine(&harness, &database, &client, &body, "machine-original").await;
    assert_eq!(retained.owner, owner.as_str());
    assert_eq!(
        retained.input.resolved_image.as_deref(),
        Some(image_a.as_str())
    );
    let counts = recovery::resource_counts(&database).await;
    let runtimes = harness.backend.inventory().await.unwrap();

    // Rebuild the service and its Mongo stores over the surviving database.
    harness.store = Arc::new(
        MongoSandboxStore::open(&client, database.name())
            .await
            .unwrap(),
    );
    harness.registry = Arc::new(LiveAgentRegistry::default());
    let (service, drain) = configured_router_for_image(
        &harness,
        &database,
        &client,
        Some(task_experience_policy()),
        true,
        image_b.clone(),
    )
    .await;
    let (status, receipt) = create(&service, &body, &retained.key).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    assert_eq!(receipt["id"], retained.id);
    let replay = waiting_record(&database, &retained.id, retained.attempts).await;
    drain.drain(Duration::from_secs(5)).await.unwrap();
    assert_eq!(replay.owner, retained.owner);
    assert_eq!(replay.input.resolved_image, retained.input.resolved_image);
    assert_eq!(recovery::resource_counts(&database).await, counts);
    assert_eq!(harness.backend.inventory().await.unwrap(), runtimes);

    let (service, drain) = configured_router_for_image(
        &harness,
        &database,
        &client,
        Some(task_experience_policy()),
        true,
        image_b.clone(),
    )
    .await;
    let (status, receipt) = create(&service, &body, "machine-fresh").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let fresh_id = receipt["id"].as_str().unwrap();
    assert_ne!(fresh_id, retained.id);
    let fresh = waiting_record(&database, fresh_id, 0).await;
    drain.drain(Duration::from_secs(5)).await.unwrap();
    assert_eq!(fresh.owner, owner.as_str());
    assert_eq!(fresh.input.resolved_image, Some(image_b));
    let original = MongoAgentLifecycleStore::open(&database)
        .await
        .unwrap()
        .read_for_owner(&owner, &retained.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.input.resolved_image, Some(image_a));
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn retained_machine_message_rechecks_task_home_but_nonmachine_replay_is_preserved() {
    let (client, database, harness) = isolated_environment().await;
    for machine in [true, false] {
        let body = json!({"name":"Retained task", "machine_included":machine, "initial_message":"Inspect workspace"});
        let key = if machine {
            "machine-task"
        } else {
            "ordinary-task"
        };
        let retained = retain_machine(&harness, &database, &client, &body, key).await;
        let configured = if machine {
            Some(policy("owner", &["desktop_enable"]))
        } else {
            None
        };
        let (service, drain) =
            configured_router(&harness, &database, &client, configured, true).await;
        if machine {
            assert_retained_refusal(
                &service,
                drain,
                &database,
                &harness,
                &retained,
                &body,
                StatusCode::FORBIDDEN,
            )
            .await;
        } else {
            let (status, receipt) = create(&service, &body, key).await;
            assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
            assert_eq!(receipt["id"], retained.id);
            waiting_record(&database, &retained.id, retained.attempts).await;
            drain.drain(Duration::from_secs(5)).await.unwrap();
        }
    }
    database.drop().await.unwrap();
}
