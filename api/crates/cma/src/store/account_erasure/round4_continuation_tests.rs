//! Declared children distinguish deleting personal roots from surviving roots.
use super::*;
use crate::store::trigger_definitions::integration_tests::{seeded, Seeded};
use layer2_core::{
    account_deletion::AccountDeletionStore,
    profiles::{
        AgentProfileSnapshot, AgentProfileStore, SandboxProfileSnapshot, SandboxProfileStore,
        WorkspaceProfileSnapshot, WorkspaceProfileStore,
    },
    resource_access::{
        OrganizationOwnershipEvidence, ResourceActor, ResourceOwner, ResourcePolicyRecord,
    },
    sandbox::{
        store::{
            AgentAdmission, AgentAdmissionStore, SandboxStore, SandboxStoreError,
            WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionStore, WorkspaceStore,
        },
        AgentOrigin, Sandbox, SandboxOrigin, SandboxStatus, SandboxWorkState, SandboxWorkStep,
        Workspace, WorkspaceOrigin, WorkspaceSource, WorkspaceStatus, WorkspaceWorkState,
        WorkspaceWorkStep,
    },
    AgentId, RuntimeCapabilities, RuntimeCapability, WorkspaceId, WorktreeId,
};
use mongodb::bson::{doc, Document};

const NOW: &str = "2026-11-14T00:00:00Z";

#[derive(Clone, Copy)]
enum Ownership {
    DeletingPerson,
    SurvivingPerson,
    Organization,
}

struct Fixture {
    seed: Seeded,
    db: Database,
    store: crate::store::MongoSandboxStore,
    parent: Sandbox,
}

impl Fixture {
    async fn new(ownership: Ownership) -> Self {
        let seed = seeded("round4_declared_children").await;
        let db = seed.client.database(&seed.database);
        let store = crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
            .await
            .unwrap();
        let exact = seed
            .profiles
            .sandbox_profile_revision(&seed.target.sandbox_profile.id, 1)
            .await
            .unwrap()
            .unwrap();
        let mut parent = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
            "sbx_round4_installing",
        );
        parent.owner = match ownership {
            Ownership::SurvivingPerson => SubjectId::new("visitor"),
            // A verified organization may overlap the deleting personal subject
            // namespace; the policy, rather than the string, proves ownership.
            Ownership::DeletingPerson | Ownership::Organization => seed.owner.clone(),
        };
        parent.origin = SandboxOrigin::Manual;
        parent.profile = SandboxProfileSnapshot::from(&exact);
        parent.status = SandboxStatus::Installing;
        parent.work = SandboxWorkState::new(SandboxWorkStep::CreateProfileWorkspace(0), NOW.into());
        parent.validate().unwrap();
        store.create_sandbox(parent.clone()).await.unwrap();
        if matches!(ownership, Ownership::Organization) {
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
                .insert_one(
                    crate::store::resource_access::records::policy_document(&policy).unwrap(),
                )
                .await
                .unwrap();
        }
        Self {
            seed,
            db,
            store,
            parent,
        }
    }

    async fn workspace(&self) -> Workspace {
        let exact = self
            .seed
            .profiles
            .workspace_profile_revision(&self.seed.target.workspace_profile.id, 1)
            .await
            .unwrap()
            .unwrap();
        Workspace {
            id: WorkspaceId::generate(),
            sandbox_id: self.parent.id.clone(),
            owner: self.parent.owner.clone(),
            origin: WorkspaceOrigin::SandboxProfile { index: 0 },
            profile: WorkspaceProfileSnapshot::from(&exact),
            name: "project".into(),
            path: "/home/cma/project".into(),
            source: WorkspaceSource::Empty,
            resolved_commit: None,
            status: WorkspaceStatus::Preparing,
            work: WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into()),
            next_worktree_number: 1,
            created_at: NOW.into(),
            ready_at: None,
            failed: None,
            removed_at: None,
            revision: 1,
        }
    }

    async fn admit_workspace(&self, workspace: Workspace) -> Result<(), SandboxStoreError> {
        self.store
            .admit_catalogue_workspace(WorkspaceAdmission {
                binding: None,
                workspace,
                kind: WorkspaceAdmissionKind::SandboxProfile,
                event_at: NOW.into(),
            })
            .await
            .map(|_| ())
    }

    async fn ready_workspace(&mut self) -> Workspace {
        let mut workspace = self.workspace().await;
        self.admit_workspace(workspace.clone()).await.unwrap();
        workspace.status = WorkspaceStatus::Ready;
        workspace.resolved_commit = Some("a".repeat(40));
        workspace.ready_at = Some(NOW.into());
        workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
        workspace.revision += 1;
        self.store
            .save_workspace(1, workspace.clone())
            .await
            .unwrap();
        let expected = self.parent.revision;
        self.parent.status = SandboxStatus::Active;
        self.parent.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
        self.parent.revision += 1;
        self.store
            .save_sandbox(expected, self.parent.clone())
            .await
            .unwrap();
        workspace
    }

    async fn agent(&self, workspace: &Workspace) -> AgentAdmission {
        let exact = self
            .seed
            .profiles
            .agent_profile_revision(&self.seed.target.agent_profile.id, 1)
            .await
            .unwrap()
            .unwrap();
        AgentAdmission {
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
        }
    }

    async fn request_owner_deletion(&self) {
        let deletions = crate::store::account_deletions::MongoAccountDeletionStore::open(&self.db)
            .await
            .unwrap();
        deletions
            .request(&self.parent.owner, "round4-owner-deletion", NOW)
            .await
            .unwrap();
        assert!(deletions.erasing(&self.parent.owner).await.unwrap());
    }

    async fn fence_profiles(&self) -> Cursor {
        let mut cursor = Cursor::default();
        let mut batch = Batch::default();
        assert!(!barrier::ready(
            &self.db,
            &[Target {
                subject: &self.seed.owner,
                inventory: &ErasureInventory::default(),
            }],
            &mut cursor,
            Options {
                allocation_rollups_enabled: Some(false),
                agent_shared_observation: Some(false),
                retain_legal_consents: false,
                drain_qualification: Some(TransactionDrainQualification::fixture()),
                billing_enabled: false,
                transaction_drain_minimum_secs: 120,
            },
            &mut batch,
        )
        .await
        .unwrap());
        assert!(batch.waiting && cursor.retained_profiles.is_none());
        for name in ["sandbox_profiles", "workspace_profiles", "agent_profiles"] {
            let row = self
                .db
                .collection::<Document>(name)
                .find_one(doc! {})
                .await
                .unwrap()
                .unwrap();
            assert!(row.get_bool("account_erasure_fenced").unwrap());
            assert!(!row.contains_key("account_erasure_retained"));
        }
        cursor
    }

    async fn source_rows(&self) -> Vec<Document> {
        let mut rows = Vec::new();
        for name in ["sandbox_profiles", "workspace_profiles", "agent_profiles"] {
            rows.push(
                self.db
                    .collection::<Document>(name)
                    .find_one(doc! {})
                    .await
                    .unwrap()
                    .unwrap(),
            );
        }
        rows
    }

    async fn finish_erasure(&self, mut cursor: Cursor) {
        cursor.drain_until = Some(mongodb::bson::DateTime::from_millis(0));
        let mut inventory = ErasureInventory::default();
        let mut complete = false;
        for _ in 0..1000 {
            let batch = erase_records(
                &self.db,
                &[Target {
                    subject: &self.seed.owner,
                    inventory: &inventory,
                }],
                &mut cursor,
                NOW,
            )
            .await
            .unwrap();
            progress::acknowledge(&mut inventory, &batch, self.seed.owner.as_str());
            if !batch.has_more {
                complete = true;
                break;
            }
        }
        assert!(complete, "erasure must finish before retained continuation");
        assert!(self
            .store
            .get_sandbox(&self.parent.id)
            .await
            .unwrap()
            .is_some());
        for row in self.source_rows().await {
            assert_eq!(
                row.get_str("owner").unwrap(),
                layer2_core::DELETED_ACCOUNT_SUBJECT
            );
            assert!(row.get_bool("account_erasure_retained").unwrap());
        }
    }
}

async fn workspace_waits(ownership: Ownership, request_deletion: bool) {
    let fixture = Fixture::new(ownership).await;
    if request_deletion {
        fixture.request_owner_deletion().await;
    }
    fixture.fence_profiles().await;
    let before = fixture.source_rows().await;
    let result = fixture.admit_workspace(fixture.workspace().await).await;
    assert!(
        matches!(result, Err(SandboxStoreError::Unavailable(_))),
        "a deleting owner's unretained declaration must stay pending: {result:?}"
    );
    assert_eq!(
        fixture
            .db
            .collection::<Document>("workspaces")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        fixture.source_rows().await,
        before,
        "admission must not retain or fence source Profiles"
    );
    fixture.db.drop().await.unwrap();
}

async fn agent_waits(ownership: Ownership, request_deletion: bool) {
    let mut fixture = Fixture::new(ownership).await;
    let workspace = fixture.ready_workspace().await;
    if request_deletion {
        fixture.request_owner_deletion().await;
    }
    fixture.fence_profiles().await;
    let before = fixture.source_rows().await;
    let result = fixture
        .store
        .admit_catalogue_agent(fixture.agent(&workspace).await)
        .await;
    assert!(
        matches!(result, Err(SandboxStoreError::Unavailable(_))),
        "a deleting owner's unretained Agent declaration must stay pending: {result:?}"
    );
    for name in ["agents", "worktrees", "agent_log"] {
        assert_eq!(
            fixture
                .db
                .collection::<Document>(name)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    assert_eq!(
        fixture.source_rows().await,
        before,
        "admission must not retain or fence source Profiles"
    );
    fixture.db.drop().await.unwrap();
}

async fn surviving_continuation(ownership: Ownership, finish_erasure: bool) {
    let mut fixture = Fixture::new(ownership).await;
    if matches!(ownership, Ownership::Organization) {
        fixture.request_owner_deletion().await;
    }
    let cursor = fixture.fence_profiles().await;
    if finish_erasure {
        fixture.finish_erasure(cursor).await;
    }
    let before = fixture.source_rows().await;
    let workspace = fixture.ready_workspace().await;
    let admission = fixture.agent(&workspace).await;
    fixture
        .store
        .admit_catalogue_agent(admission.clone())
        .await
        .unwrap();
    fixture
        .store
        .admit_catalogue_agent(admission)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .db
            .collection::<Document>("agents")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        fixture.source_rows().await,
        before,
        "declared continuation must remain read-only for source Profiles"
    );
    fixture.db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn own_installing_sandbox_declared_workspace_waits_for_retention_with_owner_fence() {
    workspace_waits(Ownership::DeletingPerson, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn own_sandbox_declared_agent_waits_for_retention_with_owner_fence() {
    agent_waits(Ownership::DeletingPerson, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn deleting_profile_consumer_declared_workspace_waits_for_retention() {
    workspace_waits(Ownership::SurvivingPerson, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn deleting_profile_consumer_declared_agent_waits_for_retention() {
    agent_waits(Ownership::SurvivingPerson, true).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn surviving_personal_sandbox_finishes_declared_children_during_source_erasure() {
    surviving_continuation(Ownership::SurvivingPerson, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn verified_organization_with_overlapping_subject_finishes_declared_children_during_erasure()
{
    surviving_continuation(Ownership::Organization, false).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn verified_organization_with_overlapping_subject_finishes_declared_children_after_erasure() {
    surviving_continuation(Ownership::Organization, true).await;
}
