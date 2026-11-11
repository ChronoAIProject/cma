//! Both reuse branches must fence before selecting/updating an existing Agent.
use super::*;
use layer2_core::profiles::{content_digest, AgentProfileRef};
use layer2_core::sandbox::store::{AgentAdmission, AgentAdmissionResult, AgentAdmissionStore};
use layer2_core::sandbox::AgentOrigin;
use layer2_core::AgentProfileId;

async fn requests(
    store: &MongoSandboxStore,
    source: &layer2_core::sandbox::ledger_maintenance_plan::LedgerMaintenanceSnapshotV1,
) -> (AgentAdmission, AgentAdmission) {
    let Request::Agent(mut automatic) = request(source, true) else {
        unreachable!()
    };
    automatic.profile.id = AgentProfileId::generate();
    let mut workspace = source.workspaces[0].clone();
    workspace.profile.content.agent_profiles = vec![AgentProfileRef::new(
        automatic.profile.id.clone(),
        automatic.profile.revision,
    )];
    workspace.profile.digest = content_digest(&workspace.profile.content).unwrap();
    store
        .workspaces
        .replace_one(
            doc! {"_id":workspace.id.as_str()},
            crate::store::workspaces::workspace_document(&workspace).unwrap(),
        )
        .await
        .unwrap();
    automatic.origin = AgentOrigin::WorkspaceProfile { index: 0 };
    automatic.creation_key = format!("workspace-profile:{}:0", workspace.id);
    let mut trigger = automatic.clone();
    trigger.agent_id = layer2_core::AgentId::generate();
    trigger.worktree_id = layer2_core::WorktreeId::generate();
    trigger.origin = AgentOrigin::Trigger {
        trigger_id: "fixture-trigger".into(),
        launch_id: "fixture-launch".into(),
    };
    trigger.creation_key = format!(
        "trigger-profile:{}",
        content_digest(&trigger.origin).unwrap()
    );
    (automatic, trigger)
}

#[tokio::test]
#[ignore = "requires an owned disposable MongoDB replica set and creation hooks"]
async fn both_profile_reuse_branches_have_positive_controls_and_held_zero_write_refusals() {
    for explicit in [false, true] {
        for held in [false, true] {
            let (client, name, store) = database().await;
            let (policy, mut input) = fixture(&store).await;
            let (automatic, trigger) = requests(&store, &input.accepted.plan().source).await;
            let (first, next) = if explicit {
                (automatic, trigger)
            } else {
                (trigger, automatic)
            };
            let original = match store.admit_agent(first).await.unwrap() {
                AgentAdmissionResult::Created(creation) => creation,
                _ => panic!("expected a newly created profile fixture"),
            };
            refresh(&store, &mut input).await;
            if held {
                store.admit_pre_exclusion(policy, input).await.unwrap();
            }
            let before = all_rows(&store).await;
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let result = if explicit {
                reopened.admit_agent_reusing_profile(next).await
            } else {
                reopened.admit_agent(next).await
            };
            if held {
                assert_eq!(result, Err(SandboxStoreError::Conflict));
                assert_eq!(all_rows(&store).await, before);
            } else {
                let AgentAdmissionResult::Existing(reused) = result.unwrap() else {
                    panic!("positive control created a different Agent instead of reusing")
                };
                assert_eq!(reused.agent.id, original.agent.id);
                assert_eq!(reused.worktree.id, original.worktree.id);
                assert_ne!(all_rows(&store).await, before, "reuse performs real writes");
            }
            client.database(&name).drop().await.unwrap();
        }
    }
}
