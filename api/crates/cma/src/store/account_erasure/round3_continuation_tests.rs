//! Existing immutable parent declarations remain authority through erasure.
use super::*;
use crate::store::trigger_definitions::integration_tests::seeded;
use futures_util::TryStreamExt;
use layer2_core::{
    profiles::{
        AgentProfileSnapshot, AgentProfileStore, SandboxProfileSnapshot, SandboxProfileStore,
    },
    resource_access::{
        OrganizationOwnershipEvidence, ResourceActor, ResourceOwner, ResourcePolicyRecord,
    },
    sandbox::{
        self,
        store::{
            AgentAdmission, AgentAdmissionStore, AgentLogStore, AgentStore, SandboxStore,
            SandboxStoreError, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionStore,
            WorkspaceStore,
        },
        AgentLogKind, AgentOrigin, AgentStatus, AgentWorkState, AgentWorkStep, SandboxOrigin,
        SandboxStatus, SandboxWorkState, SandboxWorkStep, WorkspaceOrigin, WorkspaceStatus,
        WorkspaceWorkState, WorkspaceWorkStep,
    },
    AgentId, RuntimeCapabilities, RuntimeCapability, WorktreeId,
};
use mongodb::bson::{doc, Document};
use std::sync::Arc;
const NOW: &str = "2026-11-14T00:00:00Z";

pub(super) struct NoRuntime;
#[async_trait::async_trait]
impl sandbox::AgentRuntimeRegistry for NoRuntime {
    async fn resolve(
        &self,
        _: &sandbox::AgentRuntimeTarget,
    ) -> Result<sandbox::AgentRuntimeConnection, sandbox::AgentRuntimeError> {
        Err(sandbox::AgentRuntimeError::NotFound)
    }
    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), sandbox::AgentRuntimeError> {
        Ok(())
    }
}

async fn continuation(organization: bool, finish_erasure: bool) {
    let seed = seeded("round3_existing_declared_children").await;
    let db = seed.client.database(&seed.database);
    let store = Arc::new(
        crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
            .await
            .unwrap(),
    );
    let revision = seed
        .profiles
        .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let mut parent = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
        "sbx_existing_installation",
    );
    parent.owner = SubjectId::new(if organization {
        "organization"
    } else {
        "visitor"
    });
    parent.origin = SandboxOrigin::Manual;
    parent.profile = SandboxProfileSnapshot::from(&revision);
    parent.status = SandboxStatus::Installing;
    parent.work = SandboxWorkState::new(SandboxWorkStep::CreateProfileWorkspace(0), NOW.into());
    parent.validate().unwrap();
    store.create_sandbox(parent.clone()).await.unwrap();
    if organization {
        let policy = ResourcePolicyRecord {
            sandbox_id: parent.id.clone(),
            sandbox_created_at: parent.created_at.clone(),
            owner: ResourceOwner::Organization {
                subject: parent.owner.clone(),
            },
            organization_evidence: Some(OrganizationOwnershipEvidence {
                issuer: "cma:authenticated-provider-binding-manager".into(),
                organization: parent.owner.clone(),
                admitted_by: ResourceActor::Api {
                    subject: SubjectId::new("unrelated-organization-administrator"),
                    delegation_id: None,
                },
                verified_at: NOW.into(),
            }),
            revision: 1,
        };
        db.collection::<Document>("resource_policies")
            .insert_one(crate::store::resource_access::records::policy_document(&policy).unwrap())
            .await
            .unwrap();
    }
    let mut cursor = Cursor::default();
    let mut inventory = ErasureInventory::default();
    if finish_erasure {
        let mut complete = false;
        for _ in 0..1000 {
            let batch = erase_records(
                &db,
                &[Target {
                    subject: &seed.owner,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await
            .unwrap();
            progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
            if !batch.has_more {
                complete = true;
                break;
            }
        }
        assert!(
            complete,
            "erasure must finish before testing retained continuations"
        );
        assert_eq!(
            seed.profiles
                .agent_profile(&seed.target.agent_profile.id)
                .await
                .unwrap()
                .unwrap()
                .owner
                .as_str(),
            layer2_core::DELETED_ACCOUNT_SUBJECT
        );
    } else {
        let mut batch = Batch::default();
        assert!(!barrier::ready(
            &db,
            &[Target {
                subject: &seed.owner,
                inventory: &inventory
            }],
            &mut cursor,
            Options {
                allocation_rollups_enabled: Some(false),
                agent_shared_observation: Some(false),
                retain_legal_consents: false,
                drain_qualification: Some(TransactionDrainQualification::fixture()),
                billing_enabled: false,
                transaction_drain_minimum_secs: 120
            },
            &mut batch
        )
        .await
        .unwrap());
        assert!(batch.waiting);
    }
    let workspaces = sandbox::WorkspaceService::new(store.clone(), seed.profiles.clone());
    let reference = &parent.profile.content.workspace_profiles[0];
    let mut workspace = workspaces
        .create_profile_workspace(&parent, reference, 0)
        .await
        .expect("an existing Sandbox declaration survives a fenced or retired Profile");
    let mut wrong = reference.clone();
    wrong.revision += 1;
    assert!(matches!(
        workspaces
            .create_profile_workspace(&parent, &wrong, 0)
            .await,
        Err(sandbox::WorkspaceServiceError::OriginConflict)
    ));
    let expected_parent = parent.revision;
    parent.status = SandboxStatus::Active;
    parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    parent.revision += 1;
    store
        .save_sandbox(expected_parent, parent.clone())
        .await
        .unwrap();
    let mut manual = workspace.clone();
    manual.id = layer2_core::WorkspaceId::generate();
    manual.name = "new-manual-reference".into();
    manual.path = "/home/cma/new-manual-reference".into();
    manual.origin = WorkspaceOrigin::Manual;
    assert!(matches!(
        store
            .admit_catalogue_workspace(WorkspaceAdmission {
                binding: None,
                workspace: manual,
                kind: WorkspaceAdmissionKind::Manual,
                event_at: NOW.into()
            })
            .await,
        Err(SandboxStoreError::Conflict)
    ));
    let expected_workspace = workspace.revision;
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(NOW.into());
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    workspace.revision += 1;
    store
        .save_workspace(expected_workspace, workspace.clone())
        .await
        .unwrap();
    let exact = seed
        .profiles
        .agent_profile_revision(&seed.target.agent_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let automatic = AgentAdmission {
        binding: None,
        created_by: None,
        owner: workspace.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: AgentId::generate(),
        worktree_id: WorktreeId::generate(),
        profile: AgentProfileSnapshot::from(&exact),
        capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
        title: None,
        first_message: None,
        origin: AgentOrigin::WorkspaceProfile { index: 0 },
        creation_key: format!("workspace-profile:{}:0", workspace.id),
        event_at: NOW.into(),
    };
    let mut manual = automatic.clone();
    manual.origin = AgentOrigin::Manual;
    manual.creation_key = "manual-reference-after-fence".into();
    assert!(matches!(
        store.admit_catalogue_agent(manual).await,
        Err(SandboxStoreError::Conflict)
    ));
    let agents = sandbox::AgentService::new(
        store.clone(),
        seed.profiles.clone(),
        Arc::new(NoRuntime),
        RuntimeCapabilities::new([RuntimeCapability::Conversation]),
    );
    assert_eq!(
        agents
            .reconcile_profile_agents(&parent.id)
            .await
            .expect("the existing Workspace's exact declared Agents remain admissible"),
        1
    );
    assert_eq!(
        agents.reconcile_profile_agents(&parent.id).await.unwrap(),
        0,
        "reconciliation remains idempotent"
    );
    if !finish_erasure {
        let mut complete = false;
        cursor.drain_until = Some(mongodb::bson::DateTime::from_millis(0));
        for _ in 0..1000 {
            let batch = erase_records(
                &db,
                &[Target {
                    subject: &seed.owner,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await
            .unwrap();
            progress::acknowledge(&mut inventory, &batch, seed.owner.as_str());
            if !batch.has_more {
                complete = true;
                break;
            }
        }
        assert!(
            complete,
            "erasure must finish after the declared children commit"
        );
    }
    assert!(store.get_sandbox(&parent.id).await.unwrap().is_some());
    assert!(seed
        .profiles
        .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
        .await
        .unwrap()
        .is_some());
    assert!(seed
        .profiles
        .agent_profile_revision(&seed.target.agent_profile.id, 1)
        .await
        .unwrap()
        .is_some());
    // Complete the admitted Agents' bootstrap, then exercise actual committed
    // maintenance rather than an intentionally aborted busy-Agent preview.
    let rows: Vec<Document> = db
        .collection::<Document>("agents")
        .find(doc! {"sandbox_id":parent.id.as_str()})
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    for row in rows {
        let mut agent = crate::store::agents::read_agent(&row).unwrap();
        let expected = agent.revision;
        agent.status = AgentStatus::Ready;
        agent.work = AgentWorkState::new(AgentWorkStep::None, NOW.into());
        agent.ready_at = Some(NOW.into());
        let conversation_id = format!("declared-{}", agent.id);
        agent.conversation.conversation_id = Some(conversation_id.clone());
        agent.conversation.runtime_generation = Some(1);
        agent.revision += 1;
        store.save_agent(expected, agent.clone()).await.unwrap();
        store
            .append_agent_log(
                &agent.id,
                NOW.into(),
                AgentLogKind::ConversationOpened {
                    conversation_id,
                    runtime_generation: 1,
                },
            )
            .await
            .unwrap();
    }
    use sandbox::store::{SandboxInactivityPauseRequest, SandboxMaintenanceStore};
    // Profile Agent admission records the owner's activity on the Sandbox at
    // admission time; inactivity is measured a full interval after it (#1330).
    let activity = store
        .get_sandbox(&parent.id)
        .await
        .unwrap()
        .unwrap()
        .last_activity_at;
    let requested_at = layer2_core::rfc3339_at(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(
                layer2_core::seconds_from_rfc3339(&activity)
                    .unwrap()
                    .saturating_add(901)
                    .max(layer2_core::seconds_from_rfc3339("2026-11-14T01:00:00Z").unwrap()),
            ),
    );
    assert_eq!(
        store
            .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                expected_policy_revision: None,
                sandbox_id: parent.id.clone(),
                requested_at: requested_at.clone(),
                pause_after_seconds: 900,
                auto_pause_enabled: false,
                cause: Default::default(),
            })
            .await
            .unwrap(),
        sandbox::store::SandboxMaintenanceOutcome::Scheduled
    );
    // Activation deliberately ignores the historical always-keep-alive policy
    // and must still reach its retained aggregate's transactional fence.
    assert!(store
        .schedule_inactivity_pause(SandboxInactivityPauseRequest {
            expected_policy_revision: None,
            sandbox_id: parent.id,
            requested_at,
            pause_after_seconds: 900,
            auto_pause_enabled: true,
            cause: Default::default(),
        })
        .await
        .is_ok());
    let policy = db
        .collection::<Document>("sandbox_profiles")
        .find_one(doc! {"_id":seed.target.sandbox_profile.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(
        policy.get_i64("inactivity_fence").unwrap() >= 1,
        "maintenance must reach the retained Profile policy fence"
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn installing_personal_sandbox_finishes_declared_children_during_erasure() {
    continuation(false, false).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn installing_personal_sandbox_finishes_declared_children_after_erasure() {
    continuation(false, true).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_owned_sandbox_finishes_declared_children_during_erasure() {
    continuation(true, false).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn organization_owned_sandbox_finishes_declared_children_after_erasure() {
    continuation(true, true).await;
}
