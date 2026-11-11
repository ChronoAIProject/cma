//! Child of the existing real common-launch fixture module.
use super::*;
use crate::store::ledger_maintenance_test_support::{all_rows, pre_exclusion, retain_home};
use layer2_core::sandbox::store::ledger_maintenance_pre_exclusion::LedgerPreExclusionStore;
use layer2_core::sandbox::store::{AgentAdmissionResult, SandboxStore, SandboxStoreError};

async fn retained_parent(f: &Fixture) -> Sandbox {
    let mut parent = f
        .resources
        .get_sandbox(&f.parent.id)
        .await
        .unwrap()
        .unwrap();
    let expected = parent.revision;
    if parent.runtime.is_none() {
        parent.runtime = Some(RuntimeBinding {
            runtime_id: "fixture-runtime".into(),
            runtime_version: "fixture-image".into(),
            provision_generation: Some(1),
            boot_id: Some("b".repeat(32)),
            bound_at: layer2_core::now_rfc3339(),
        });
    }
    let runtime = parent.runtime.as_mut().unwrap();
    runtime.provision_generation = Some(1);
    runtime.boot_id = Some("b".repeat(32));
    parent.lifecycle.runtime_generation_high_water = 1;
    retain_home(&mut parent);
    parent.revision += 1;
    f.resources
        .save_sandbox(expected, parent.clone())
        .await
        .unwrap();
    parent
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn new_trigger_creation_refuses_under_hold_and_rolls_back_attribution_and_command_sidecars() {
    for held in [false, true] {
        let (f, launch, request) = pending().await;
        let parent = retained_parent(&f).await;
        if held {
            let (policy, input) =
                pre_exclusion(&f.resources, &parent, &layer2_core::now_rfc3339()).await;
            f.resources
                .admit_pre_exclusion(policy, input)
                .await
                .unwrap();
        }
        let before = all_rows(&f.resources).await;
        let result = f
            .resources
            .admit_agent_with_trigger_initiator(request, launch.initiation.unwrap(), false)
            .await;
        if held {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            assert_eq!(all_rows(&f.resources).await, before);
        } else {
            assert!(matches!(result, Ok(AgentAdmissionResult::Created(_))));
            assert_ne!(all_rows(&f.resources).await, before);
        }
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn accepted_common_launch_replays_exactly_under_hold_without_attribution_writes() {
    let (f, launch, request) = pending().await;
    let parent = retained_parent(&f).await;
    let original = match f
        .resources
        .admit_agent_with_trigger_initiator(
            request.clone(),
            launch.initiation.clone().unwrap(),
            false,
        )
        .await
        .unwrap()
    {
        AgentAdmissionResult::Created(creation) => creation,
        _ => panic!("expected a new common-launch fixture"),
    };
    let (policy, input) = pre_exclusion(&f.resources, &parent, &layer2_core::now_rfc3339()).await;
    f.resources
        .admit_pre_exclusion(policy, input)
        .await
        .unwrap();
    let before = all_rows(&f.resources).await;
    let reopened = crate::store::MongoSandboxStore::open(&f.seed.client, &f.seed.database)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .admit_agent_with_trigger_initiator(request, launch.initiation.unwrap(), false)
            .await
            .unwrap(),
        AgentAdmissionResult::Existing(original)
    );
    assert_eq!(all_rows(&f.resources).await, before);
    f.close().await;
}
