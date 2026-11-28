use super::*;
use crate::config::{FeatureFlags, ManagedMachineUpdates};
use crate::store::agent_lifecycle_operations::COLLECTION;
use crate::store::released_runtimes::MongoReleasedRuntimes;
use crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use layer1_public::rest::{hierarchy_router, HierarchyState};
use layer2_core::released_runtime::{ReleasedLane, ReleasedLaneRecord};
use layer2_core::{Authorization, Capability};
use layer3_adapter::{
    PinnedReleasedRuntime, ReleasedLaneRevisions, ReleasedRuntime, ReleasedRuntimeSource,
};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

fn image_origin_flags(machine_scope: Option<String>) -> FeatureFlags {
    FeatureFlags::from_lookup(&mut |key| {
        Ok(match key {
            "CHRONO_RUNTIME_IMAGE_ORIGIN_ENABLED" => Some("true".into()),
            "CHRONO_MANAGED_MACHINE_RUNTIME_UPDATES" => machine_scope.clone(),
            _ => None,
        })
    })
    .unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn managed_machine_origin_uses_the_exact_creation_and_native_home() {
    let (client, name, mut store) = database().await;
    let database = client.database(&name);
    let operations = crate::store::MongoAgentLifecycleStore::open(&database)
        .await
        .unwrap();
    let machine = machine_tests::machine_plan();
    let agent_release = plan().warm.runtime_version;
    let initial_release = PinnedReleasedRuntime::new(
        agent_release.clone(),
        Some(machine.warm.runtime_version.clone()),
    )
    .snapshot();
    let machine_release = format!("registry.example/omarchy@sha256:{}", "d".repeat(64));
    let releases = Arc::new(PinnedReleasedRuntime::new(
        agent_release.clone(),
        Some(machine_release.clone()),
    ));
    let defaults = image_origin_flags(None);
    assert!(defaults.runtime_image_origin_enabled);
    store.runtime_image_origin_enabled = defaults.runtime_image_origin_enabled;
    for (key, explicit, prepared) in [
        ("cold-managed", false, false),
        ("explicit-choice", true, false),
        ("prepared-managed", false, true),
    ] {
        let owner = SubjectId::new("retained-machine-owner");
        let mut input = lifecycle_input(&machine);
        input.request.desktop = true;
        if explicit {
            input.request.image = serde_json::from_value(json!({"id":"owner-image"})).unwrap();
        } else {
            assert!(input.request.image.is_none());
            let lane = crate::store::managed_lifecycle_release_lane(&input.request, input.behavior);
            assert_eq!(lane, Some(ReleasedLane::Machine));
            input.resolved_image = lane
                .and_then(|lane| lane.version_in(&initial_release))
                .map(str::to_owned);
            assert_eq!(
                input.resolved_image.as_deref(),
                Some(machine.warm.runtime_version.as_str())
            );
        }
        let mut workspace = machine.warm.workspaces[0].profile.clone();
        workspace.id =
            private_workspace_profile_id(&owner, &format!("agent-manager:{key}:workspace"))
                .unwrap();
        workspace.content.agent_profiles.clear();
        workspace.digest = content_digest(&workspace.content).unwrap();
        let reference = WorkspaceProfileRef::new(workspace.id.clone(), workspace.revision);
        let content = crate::agent_lifecycle::canary_sandbox_content(&input, reference).unwrap();
        let agent_content = machine.warm.workspaces[0].agents[0].content.clone();
        let plan = NativePreparedPlan::new(
            content.clone(),
            workspace.content.clone(),
            agent_content.clone(),
            content.nyxid_connection_services(),
            machine.warm.runtime_version.clone(),
            machine.warm.execution,
            machine.home_policy.clone(),
        )
        .unwrap();
        let mut native = ready(Record {
            payload: WarmSandbox::new(plan.warm.clone(), NOW.into()).unwrap(),
            plan,
            ownership: None,
        });
        let mut sandbox = active_sandbox(native.payload.id.as_str());
        sandbox.owner = owner.clone();
        sandbox.runtime = native.payload.runtime.clone();
        sandbox.lifecycle.home = native.payload.home.clone();
        if prepared {
            sandbox.work.step = SandboxWorkStep::None;
        } else {
            sandbox.work.schedule(
                SandboxWorkStep::WaitForCredentialRotation,
                "2026-09-05T01:00:00Z".into(),
            );
        }
        sandbox.credential.reaches = native.plan.warm.reaches.clone();
        sandbox.profile.id =
            private_sandbox_profile_id(&sandbox.owner, &format!("agent-manager:{key}:sandbox"))
                .unwrap();
        sandbox.profile.content = content;
        sandbox.profile.digest = content_digest(&sandbox.profile.content).unwrap();
        let crate::store::CreateResult::Created(operation) =
            operations.create(&sandbox.owner, key, input).await.unwrap()
        else {
            panic!("expected a new operation");
        };
        sandbox.origin = SandboxOrigin::Lifecycle {
            operation_id: operation.clone(),
        };
        database
            .collection::<Document>(COLLECTION)
            .update_one(
                doc! {"_id": &operation},
                doc! {"$set": {"sandbox_id": sandbox.id.as_str(), "status": "ready"}},
            )
            .await
            .unwrap();
        // Seed the retained state with the same immutable admission identity
        // the lifecycle worker recorded before the runtime became Active.
        let mut fresh = crate::store::sandbox_admission_mongo_integration_tests::sandbox(
            sandbox.id.as_str(),
            key,
        );
        fresh.owner = sandbox.owner.clone();
        fresh.name = sandbox.name.clone();
        fresh.origin = sandbox.origin.clone();
        fresh.profile = sandbox.profile.clone();
        fresh.limits = sandbox.limits;
        fresh.credential.source = sandbox.credential.source.clone();
        fresh.credential.reaches = sandbox.credential.reaches.clone();
        fresh.credential.sign_in = sandbox.credential.sign_in;
        let admission = SandboxAdmission::new(
            fresh,
            SandboxAdmissionName::Exact(sandbox.name.clone()),
            4,
            NOW.into(),
        )
        .unwrap();
        let document = crate::store::sandboxes::admitted_sandbox_document(
            &sandbox,
            0,
            &admission.name,
            &admission.admission_fingerprint,
        )
        .unwrap();
        store.sandboxes.insert_one(document).await.unwrap();
        store.managed_machine_updates = defaults.managed_machine_updates;
        assert_eq!(
            store.managed_machine_updates,
            ManagedMachineUpdates::Disabled
        );
        assert!(store
            .qualify_retained_machine(&sandbox, LATER)
            .await
            .unwrap()
            .is_none());
        if key == "cold-managed" {
            let view = runtime_update(&store, releases.clone(), &sandbox).await;
            eprintln!("shared desktop, scope unset: {view}");
            assert_eq!(view["current_version"], machine.warm.runtime_version);
            assert_eq!(view["latest_version"], agent_release);
            assert_eq!(view["image_origin"], "unknown");
            assert_eq!(view["can_update"], false);
            assert_eq!(
                store.get_sandbox(&sandbox.id).await.unwrap(),
                Some(sandbox.clone())
            );
        }
        store.managed_machine_updates = if key == "cold-managed" {
            image_origin_flags(Some(sandbox.id.to_string())).managed_machine_updates
        } else {
            ManagedMachineUpdates::All
        };
        if key == "cold-managed" {
            let mut rotating = sandbox.clone();
            rotating
                .work
                .claim(
                    "rotation-worker".into(),
                    "2026-09-05T01:00:00Z",
                    "2026-09-05T01:01:00Z".into(),
                )
                .unwrap();
            assert!(store
                .qualify_retained_machine(&rotating, "2026-09-05T01:00:01Z")
                .await
                .unwrap()
                .is_none());
        }
        if prepared {
            native.payload.state = WarmSandboxState::Claimed;
            native.ownership = Some(super::super::model::Ownership {
                owner: SubjectId::new("foreign-owner"),
                operation_id: operation,
                sandbox: sandbox.profile.clone(),
                workspace,
                agent_content,
                agent: None,
            });
            store
                .native_prepared
                .insert_one(super::document(&native).unwrap())
                .await
                .unwrap();
            assert!(store
                .qualify_retained_machine(&sandbox, LATER)
                .await
                .unwrap()
                .is_none());
            native.ownership.as_mut().unwrap().owner = owner;
            store
                .native_prepared
                .replace_one(
                    doc! {"_id": sandbox.id.as_str()},
                    super::document(&native).unwrap(),
                )
                .await
                .unwrap();
        }
        let qualified = store
            .qualify_retained_machine(&sandbox, LATER)
            .await
            .unwrap();
        if explicit {
            assert!(
                qualified.is_none(),
                "the same digest cannot authorize an owner image"
            );
            assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(sandbox));
        } else {
            let qualified = qualified.expect("proved managed Machine");
            assert_eq!(
                qualified.managed_runtime_lane(),
                Some(ReleasedLane::Machine)
            );
            assert_eq!(qualified.profile, sandbox.profile);
            assert_eq!(qualified.lifecycle.home, sandbox.lifecycle.home);
            assert_eq!(qualified.runtime, sandbox.runtime);
            assert_eq!(qualified.work, sandbox.work);
            assert_eq!(
                store.get_sandbox(&sandbox.id).await.unwrap(),
                Some(qualified.clone())
            );
            if key == "cold-managed" {
                let view = runtime_update(&store, releases.clone(), &qualified).await;
                eprintln!("shared desktop, scope matches: {view}");
                assert_eq!(view["current_version"], machine.warm.runtime_version);
                assert_eq!(view["latest_version"], machine_release);
                assert_eq!(view["image_origin"], "managed_machine");
                assert_eq!(view["current_version_confirmed"], true);
                assert_eq!(view["can_update"], true);
            }
            crate::store::work_schema::floor::verify(&database)
                .await
                .unwrap();
            if prepared {
                verify_machine_release(&database, &mut store, qualified).await;
            }
        }
    }
    database.drop().await.unwrap();
}

async fn runtime_update(
    store: &MongoSandboxStore,
    releases: Arc<PinnedReleasedRuntime>,
    sandbox: &Sandbox,
) -> Value {
    let store = Arc::new(store.clone());
    let profiles = Arc::new(InMemoryProfileStore::default());
    let operations = crate::store::operation::MongoOperationStore::open(
        &store.client,
        &store.sandboxes.namespace().db,
    )
    .await
    .unwrap();
    let app = hierarchy_router(
        HierarchyState::new(
            SandboxService::new(
                store.clone(),
                profiles.clone(),
                SandboxPolicy::default(),
                Arc::new(FixedSandboxCredentialPrerequisite::Ready),
            ),
            WorkspaceService::new(store.clone(), profiles),
            HierarchyReadService::new(store.clone()),
            Arc::new(operations),
        )
        .with_runtime_updates(Arc::new(SandboxRuntimeUpdateService::with_released(
            store, releases,
        ))),
    );
    let mut request = Request::builder()
        .uri(format!("/api/v1/sandboxes/{}", sandbox.id))
        .body(Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(sandbox.owner.clone()));
    request
        .extensions_mut()
        .insert(Authorization::new([Capability::UseOwnSandboxes]));
    let response = app.oneshot(request).await.unwrap();
    let status = response.status();
    let body: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    body["runtime_update"].clone()
}

async fn verify_machine_release(
    database: &mongodb::Database,
    store: &mut MongoSandboxStore,
    sandbox: Sandbox,
) {
    let target = format!("registry.example/omarchy@sha256:{}", "d".repeat(64));
    let lanes = MongoReleasedRuntimes::open(database, "retained-machine-fixture");
    lanes.initialize().await.unwrap();
    lanes
        .bootstrap(
            ReleasedLaneRecord::bootstrap(ReleasedLane::Machine, target.clone(), LATER).unwrap(),
        )
        .await
        .unwrap();
    store.released_lanes = Some(lanes);
    let release = |machine_revision| ReleasedRuntime {
        revision: 1,
        agent: "unrelated-agent-image".into(),
        machine: Some(target.clone()),
        lane_revisions: Some(ReleasedLaneRevisions {
            agent: 1,
            machine: Some(machine_revision),
        }),
    };
    let mut next = sandbox.clone();
    assert!(next
        .request_runtime_update(target.clone(), false, LATER.into())
        .unwrap());
    assert!(matches!(
        store
            .save_sandbox_against_release(
                sandbox.revision,
                next.clone(),
                &release(0),
                ReleasedLane::Machine,
            )
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap().as_ref(),
        Some(&sandbox)
    );
    store
        .save_sandbox_against_release(
            sandbox.revision,
            next.clone(),
            &release(1),
            ReleasedLane::Machine,
        )
        .await
        .unwrap();
    assert_eq!(store.get_sandbox(&next.id).await.unwrap(), Some(next));
}
