use super::*;
use layer2_core::released_runtime::{ReleasedLane, ReleasedLaneRecord};
use layer2_core::sandbox::store::{SandboxAdmission, SandboxAdmissionName};
use layer2_core::sandbox::{Sandbox, SandboxOrigin, SandboxRuntimeImageOrigin};
use layer3_adapter::ReleasedLaneRevisions;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn machine_reopen_refusal_parks_only_at_the_current_machine_revision() {
    run_case(2, true, true).await;
}

pub(super) async fn setup_release(
    database: &mongodb::Database,
    store: &mut MongoSandboxStore,
    parent: &mut Sandbox,
) -> ReleasedRuntime {
    let lanes =
        crate::store::released_runtimes::MongoReleasedRuntimes::open(database, "reopen-fixture");
    lanes.initialize().await.unwrap();
    let agent_image = format!("registry.example/agent@sha256:{}", "a".repeat(64));
    let machine_image = format!("registry.example/machine@sha256:{}", "b".repeat(64));
    let at = layer2_core::now_rfc3339();
    for (lane, image) in [
        (ReleasedLane::Agent, &agent_image),
        (ReleasedLane::Machine, &machine_image),
    ] {
        lanes
            .bootstrap(ReleasedLaneRecord::bootstrap(lane, image.clone(), &at).unwrap())
            .await
            .unwrap();
    }
    store.released_lanes = Some(lanes);
    parent.origin = SandboxOrigin::Lifecycle {
        operation_id: "prepared-machine".into(),
    };
    parent.profile.content.runtime_image = Some("image-v1".into());
    parent.profile.digest = layer2_core::profiles::content_digest(&parent.profile.content).unwrap();
    parent.lifecycle.runtime_image_origin = Some(SandboxRuntimeImageOrigin::ManagedMachine);
    install_admission(store, parent, &at).await;
    ReleasedRuntime {
        revision: 1,
        agent: agent_image,
        machine: Some(machine_image),
        lane_revisions: Some(ReleasedLaneRevisions {
            agent: 1,
            machine: Some(1),
        }),
    }
}

async fn install_admission(store: &MongoSandboxStore, parent: &Sandbox, at: &str) {
    let mut fresh = crate::store::sandbox_admission_mongo_integration_tests::sandbox(
        parent.id.as_str(),
        "prepared-machine",
    );
    fresh.owner = parent.owner.clone();
    fresh.origin = parent.origin.clone();
    fresh.name = parent.name.clone();
    fresh.profile = parent.profile.clone();
    fresh.limits = parent.limits;
    fresh.credential.source = parent.credential.source.clone();
    fresh.credential.reaches = parent.credential.reaches.clone();
    fresh.credential.sign_in = parent.credential.sign_in;
    let admission = SandboxAdmission::new(
        fresh,
        SandboxAdmissionName::Exact(parent.name.clone()),
        4,
        at.into(),
    )
    .unwrap();
    let row = crate::store::sandboxes::admitted_sandbox_document(
        parent,
        0,
        &admission.name,
        &admission.admission_fingerprint,
    )
    .unwrap();
    store
        .sandboxes
        .replace_one(doc! {"_id": parent.id.as_str()}, row)
        .await
        .unwrap();
}

pub(super) async fn assert_stale_release_refused(
    store: &MongoSandboxStore,
    mutation: &SandboxLifecycleMutation,
    fence: &ReopenRefusalFence,
) {
    if let Some(released) = &fence.released {
        let mut stale = fence.clone();
        stale.released.as_mut().unwrap().lane_revisions = Some(ReleasedLaneRevisions {
            machine: Some(0),
            ..released.lane_revisions.unwrap()
        });
        assert_eq!(
            store
                .park_sandbox_reopen_refusal(mutation.clone(), stale)
                .await,
            Err(SandboxStoreError::Conflict)
        );
    }
}
