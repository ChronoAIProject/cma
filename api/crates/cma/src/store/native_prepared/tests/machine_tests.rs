use super::*;

pub(super) fn machine_plan() -> NativePreparedPlan {
    let original = plan();
    let image = format!("registry.example/omarchy@sha256:{}", "c".repeat(64));
    let mut sandbox = original.warm.profile.content.clone();
    sandbox.workspace_profiles.clear();
    sandbox.runtime_image = Some(image.clone());
    sandbox.environment_variables = vec![EnvironmentVariable {
        name: "CMA_MACHINE_INCLUDED".into(),
        value: "1".into(),
    }];
    let mut workspace = original.warm.workspaces[0].profile.content.clone();
    workspace.agent_profiles.clear();
    NativePreparedPlan::new(
        sandbox,
        workspace,
        original.warm.workspaces[0].agents[0].content.clone(),
        original.warm.reaches,
        image,
        original.warm.execution,
        original.home_policy,
    )
    .unwrap()
}

pub(super) async fn ready_machine(store: &MongoSandboxStore) -> Record {
    let original = records(store)
        .await
        .into_iter()
        .find(|r| r.plan.machine() && r.payload.state == WarmSandboxState::Preparing)
        .unwrap();
    let record = ready(original);
    let mut value = document(&record).unwrap();
    value.insert("lease_until", NOW);
    value.insert("lease_token", mongodb::bson::Bson::Null);
    value.insert("check_after", LATER);
    store
        .native_prepared
        .replace_one(doc! {"_id":record.payload.id.as_str()}, value)
        .await
        .unwrap();
    record
}

pub(crate) async fn admit_machine_for_pointer_fixture(
    store: &MongoSandboxStore,
    release: &layer3_adapter::ReleasedRuntime,
    owner: &str,
    key: &str,
) -> Result<Sandbox, Box<dyn std::error::Error + Send + Sync>> {
    use layer2_core::released_runtime::ReleasedLane;

    let plan = machine_plan().for_runtime(release.machine.as_deref().expect("Machine lane"))?;
    store
        .reconcile_native_prepared_release(Some(&plan), 1, NOW, true, release)
        .await?;
    let prepared = ready_machine(store).await;
    let database = store.client.database(&store.sandboxes.namespace().db);
    let mut lifecycle = crate::store::MongoAgentLifecycleStore::open(&database).await?;
    lifecycle.released_lanes = store.released_lanes.clone();
    let result = lifecycle
        .create_with_submission(
            &SubjectId::new(owner),
            key,
            lifecycle_input(&plan),
            None,
            Some((release, ReleasedLane::Machine)),
        )
        .await?;
    let crate::store::CreateResult::Created(operation_id) = result else {
        panic!("expected a new Machine lifecycle receipt");
    };
    let request = sandbox_admission(store, &plan, owner, key, operation_id).await;
    let SandboxAdmissionResult::Created(owned) = admit_at(store, request, NOW).await? else {
        panic!("expected a newly owned Machine");
    };
    assert_eq!(owned.id, prepared.payload.id);
    assert_eq!(owned.runtime, prepared.payload.runtime);
    Ok(owned)
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn health_probe_scheduling_keeps_each_classes_other_reservation_admissible() {
    let (client, name, store) = database().await;
    let agent = plan();
    let machine = machine_plan();
    let store = store
        .with_managed_runtime_version(agent.warm.runtime_version.clone())
        .with_machine_runtime_version(Some(machine.warm.runtime_version.clone()));
    store
        .reconcile_native_prepared_target(Some(&agent), 2, NOW)
        .await
        .unwrap();
    publish_ready(&store).await;
    publish_ready(&store).await;
    store
        .reconcile_native_prepared_class(Some(&machine), 2, NOW, true)
        .await
        .unwrap();
    ready_machine(&store).await;
    ready_machine(&store).await;
    store
        .native_prepared
        .update_many(
            doc! {"kind":"reservation"},
            doc! {"$set":{"check_after":NOW}},
        )
        .await
        .unwrap();
    let first = store
        .claim_native_prepared_work_for(NOW, &lease_deadline(), [true, true])
        .await
        .unwrap()
        .unwrap();
    let mut classes = [true, true];
    classes[usize::from(first.plan.machine())] = false;
    let second = store
        .claim_native_prepared_work_for(NOW, &lease_deadline(), classes)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(first.plan.machine(), second.plan.machine());
    assert!(store
        .claim_native_prepared_work_for(NOW, &lease_deadline(), [false, false])
        .await
        .unwrap()
        .is_none());
    for (index, held) in [&first, &second].into_iter().enumerate() {
        let request = admission(&store, &held.plan, "owner", &format!("probe-spare-{index}")).await;
        let SandboxAdmissionResult::Created(owned) = admit_at(&store, request, NOW).await.unwrap()
        else {
            panic!("new admission");
        };
        assert!(owned.runtime.is_some());
        assert_ne!(owned.id, held.record.id);
    }
    // Preparation and retirement can still progress while both classes probe.
    store
        .reconcile_native_prepared_target(Some(&agent), 2, NOW)
        .await
        .unwrap();
    let preparing = store
        .claim_native_prepared_work_for(NOW, &lease_deadline(), [false, false])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(preparing.record.state, WarmSandboxState::Preparing);
    for held in [&first, &second, &preparing] {
        store
            .finish_native_prepared_work(held, held.record.clone(), NOW, LATER)
            .await
            .unwrap();
    }
    client.database(&name).drop().await.unwrap();
}

#[test]
fn machine_preparation_accepts_only_exact_marker_and_preserves_channel_authority() {
    let machine = machine_plan();
    assert!(machine.machine());
    let record = Record {
        payload: WarmSandbox::new(machine.warm.clone(), NOW.into()).unwrap(),
        plan: machine,
        ownership: None,
    };
    let mut value = document(&record).unwrap();
    assert!(read(&value).is_ok());
    value.insert("machine", false);
    assert!(read(&value).is_err());
    let mut profile = record.plan.warm.profile.content;
    profile.environment_variables[0].value = "0".into();
    assert!(!crate::sandbox::native_prepared::valid_environment(
        &profile
    ));
    profile.environment_variables[0].value = "1".into();
    profile.environment_variables.push(EnvironmentVariable {
        name: "EXTRA".into(),
        value: "1".into(),
    });
    assert!(!crate::sandbox::native_prepared::valid_environment(
        &profile
    ));
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn two_prepared_classes_have_independent_caps_and_retirement() {
    let (client, name, store) = database().await;
    let agent = plan();
    let machine = machine_plan();
    for _ in 0..3 {
        let (a, b) = tokio::join!(
            store.reconcile_native_prepared_target(Some(&agent), 2, NOW),
            store.reconcile_native_prepared_class(Some(&machine), 2, NOW, true)
        );
        a.unwrap();
        b.unwrap();
    }
    let before = records(&store).await;
    assert_eq!(before.len(), 4);
    assert_eq!(before.iter().filter(|r| r.plan.machine()).count(), 2);
    store
        .reconcile_native_prepared_target(None, 0, NOW)
        .await
        .unwrap();
    let after = records(&store).await;
    assert!(after
        .iter()
        .filter(|r| r.plan.machine())
        .all(|r| before.contains(r)));
    assert!(after
        .iter()
        .filter(|r| !r.plan.machine())
        .all(|r| r.payload.state == WarmSandboxState::Retiring));
    assert!(store
        .reconcile_native_prepared_class(Some(&agent), 1, NOW, true)
        .await
        .is_err());
    assert!(store
        .reconcile_native_prepared_class(Some(&machine), 3, NOW, true)
        .await
        .is_err());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn machine_claim_uses_prestarted_desktop_and_preserves_agent_spares() {
    let (client, name, store) = database().await;
    let agent = plan();
    let machine = machine_plan();
    let store = store
        .with_managed_runtime_version(agent.warm.runtime_version.clone())
        .with_machine_runtime_version(Some(machine.warm.runtime_version.clone()));
    store
        .reconcile_native_prepared_target(Some(&agent), 2, NOW)
        .await
        .unwrap();
    let first_agent = publish_ready(&store).await;
    let second_agent = publish_ready(&store).await;
    store
        .reconcile_native_prepared_class(Some(&machine), 1, NOW, true)
        .await
        .unwrap();
    let prepared = ready_machine(&store).await;
    let request = admission(&store, &machine, "owner", "machine-claim").await;
    let SandboxAdmissionResult::Created(owned) = admit_at(&store, request, NOW).await.unwrap()
    else {
        panic!("machine admission");
    };
    assert_eq!(owned.id, prepared.payload.id);
    assert_eq!(owned.runtime, prepared.payload.runtime);
    assert_eq!(
        owned.profile.content.environment_variables,
        machine.warm.profile.content.environment_variables
    );
    let remaining = records(&store).await;
    assert!(remaining.contains(&first_agent));
    assert!(remaining.contains(&second_agent));
    let reopened = MongoSandboxStore::open(&client, &name)
        .await
        .unwrap()
        .with_managed_runtime_version(agent.warm.runtime_version)
        .with_machine_runtime_version(Some(format!(
            "registry.example/omarchy@sha256:{}",
            "d".repeat(64)
        )));
    store
        .reconcile_native_prepared_class(Some(&machine), 1, NOW, true)
        .await
        .unwrap();
    let spare = ready_machine(&store).await;
    let request = admission(&reopened, &machine, "owner", "stale-machine-claim").await;
    let SandboxAdmissionResult::Created(cold) = admit_at(&reopened, request, NOW).await.unwrap()
    else {
        panic!("fresh admission");
    };
    assert_ne!(cold.id, spare.payload.id);
    assert!(cold.runtime.is_none());
    assert!(records(&reopened).await.contains(&spare));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn owner_selected_image_cannot_consume_a_managed_machine_member() {
    let (client, name, store) = database().await;
    let machine = machine_plan();
    let store = store.with_machine_runtime_version(Some(machine.warm.runtime_version.clone()));
    store
        .reconcile_native_prepared_class(Some(&machine), 1, NOW, true)
        .await
        .unwrap();
    let spare = ready_machine(&store).await;
    let request = admission(&store, &machine, "owner", "explicit-machine-image").await;
    let SandboxOrigin::Lifecycle { operation_id } = &request.sandbox.origin else {
        panic!("expected a lifecycle admission");
    };
    // The profile and resolved digest match the pool. An explicit owner choice
    // in a retained operation still prevents it from proving a managed admission.
    let updated = client
        .database(&name)
        .collection::<Document>(crate::store::agent_lifecycle_operations::COLLECTION)
        .update_one(
            doc! { "_id": operation_id },
            doc! { "$set": { "input.request.image": { "id": "owner-image" } } },
        )
        .await
        .unwrap();
    assert_eq!(updated.modified_count, 1);

    let SandboxAdmissionResult::Created(cold) = admit_at(&store, request, NOW).await.unwrap()
    else {
        panic!("expected a cold admission");
    };
    assert_ne!(cold.id, spare.payload.id);
    assert!(cold.runtime.is_none());
    assert!(records(&store).await.contains(&spare));
    client.database(&name).drop().await.unwrap();
}
