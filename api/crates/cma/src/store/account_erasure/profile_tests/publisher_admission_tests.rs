//! Publisher fences also guard new effects whose Profile owners still survive.
mod cutoff_tests;
mod effect_tests;

use super::*;
use crate::store::trigger_definitions::integration_tests::Seeded;
use layer2_core::sandbox::store::{
    SandboxAdmission, SandboxAdmissionName, SandboxAdmissionResult, SandboxAdmissionStore,
    SandboxStoreError, WorkspaceAdmission, WorkspaceAdmissionKind, WorkspaceAdmissionResult,
    WorkspaceAdmissionStore,
};
use layer2_core::sandbox::{
    CreateSandbox, CreateWorkspace, FixedSandboxCredentialPrerequisite, Sandbox, SandboxOrigin,
    SandboxPolicy, SandboxService, SandboxServiceError, WorkspaceService, WorkspaceServiceError,
};
use layer2_core::trigger_domain::*;
use layer2_core::{Authorization, Caller, Capability, SandboxId, ServiceId};
use std::sync::Arc;

const HOLDER: &str = "publisher-admission-worker";
const NAME: &str = "publisher-admission";

struct Connected;
#[async_trait::async_trait]
impl ServiceConnectionBroker for Connected {
    async fn requirements(
        &self,
        _: &SubjectId,
        services: &[ServiceId],
    ) -> Result<Vec<ServiceRequirement>, ServiceConnectionError> {
        Ok(services
            .iter()
            .map(|service| ServiceRequirement {
                service: service.clone(),
                state: ServiceRequirementState::Satisfied,
            })
            .collect())
    }
    async fn create_handoff(
        &self,
        _: &SubjectId,
        _: &ServiceId,
        _: &str,
    ) -> Result<BrokerHandoff, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
    async fn handoff_state(
        &self,
        _: &SubjectId,
        _: &str,
        _: &ServiceId,
    ) -> Result<BrokerHandoffState, ServiceConnectionError> {
        Err(ServiceConnectionError::Refused)
    }
}

struct Fixture {
    seed: Seeded,
    resources: Arc<crate::store::MongoSandboxStore>,
    caller: Caller,
    launch: Launch,
    sandboxes: Arc<SandboxService>,
    workspaces: Arc<WorkspaceService>,
}

impl Fixture {
    async fn new(prefix: &str) -> Self {
        let seed = seeded(prefix).await;
        let publisher = SubjectId::new("independent-trigger-publisher");
        let id = TriggerId("trg_publisher_admission".into());
        seed.store
            .create_trigger(trigger(id.as_str(), &publisher, &seed.target))
            .await
            .unwrap();
        let public = seed
            .store
            .publish_trigger(&id, &publisher, 1, layer2_core::now_rfc3339())
            .await
            .unwrap();
        let resources = Arc::new(
            crate::store::MongoSandboxStore::open(&seed.client, &seed.database)
                .await
                .unwrap(),
        );
        let definitions = Arc::new(seed.store.clone());
        let service = LaunchService::new(
            definitions.clone(),
            seed.profiles.clone(),
            definitions,
            resources.clone(),
            Arc::new(Connected),
            Arc::new(LaunchPolicy {
                enabled: true,
                orchestration_paused: false,
                visitor_launches_per_day: 5,
            }),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        );
        let caller = Caller::new(
            SubjectId::new("independent-visitor"),
            Authorization::new([
                Capability::CreateSandbox,
                Capability::LaunchTriggers,
                Capability::UseOwnSandboxes,
            ]),
        );
        let draft = service
            .create_or_resume(
                &caller,
                &public.public_id,
                CreateOrResumeLaunch {
                    placement_id: None,
                    referrer_origin: None,
                    start_if_ready: false,
                    choice: None,
                    idempotency_key: "publisher-admission".into(),
                },
            )
            .await
            .unwrap();
        let launch = service
            .start(
                &caller,
                &draft.launch.id,
                StartLaunch {
                    trust_confirmed_digest: draft.launch.review.trust_digest().map(str::to_owned),
                    acknowledgements: vec![],
                },
            )
            .await
            .unwrap();
        let sandboxes = SandboxService::new(
            resources.clone(),
            seed.profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        );
        let workspaces = WorkspaceService::new(resources.clone(), seed.profiles.clone());
        Self {
            seed,
            resources,
            caller,
            launch,
            sandboxes,
            workspaces,
        }
    }

    async fn change(
        &mut self,
        mutation: impl FnOnce(&mut Launch, u64, String) -> Vec<LaunchEventKind>,
    ) {
        let at = layer2_core::now_rfc3339();
        let mut launch = if self.launch.work.lease.is_none() {
            self.seed
                .store
                .claim_launch(
                    &self.launch.id,
                    HOLDER,
                    at.clone(),
                    layer2_core::rfc3339_at(
                        std::time::SystemTime::now() + std::time::Duration::from_secs(600),
                    ),
                )
                .await
                .unwrap()
                .unwrap()
        } else {
            self.launch.clone()
        };
        let expected_revision = launch.revision;
        let fence = launch.work.lease.as_ref().unwrap().fence;
        let events = mutation(&mut launch, fence, at.clone());
        self.launch = self
            .seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision,
                launch,
                event_at: at,
                events,
            })
            .await
            .unwrap();
    }

    async fn executing(&mut self, kind: LaunchEffectKind) {
        self.change(|launch, fence, at| {
            launch
                .prepare_worker_effect(HOLDER, fence, kind, at)
                .unwrap()
        })
        .await;
        self.change(|launch, fence, at| {
            launch
                .mark_worker_effect_executing(HOLDER, fence, at)
                .unwrap();
            vec![]
        })
        .await;
        assert_eq!(
            self.launch.work.effect.as_ref().unwrap().state,
            LaunchEffectState::Executing
        );
    }

    async fn create_sandbox(&self) -> Result<Sandbox, SandboxServiceError> {
        self.sandboxes
            .create_for_launch(
                &self.caller,
                Capability::CreateSandbox,
                CreateSandbox {
                    profile: self.seed.target.sandbox_profile.clone(),
                    name: Some(NAME.into()),
                    confirmed_digest: self.launch.trust.sandbox_digest.clone(),
                    origin: SandboxOrigin::Trigger {
                        launch_id: self.launch.id.to_string(),
                    },
                },
            )
            .await
    }

    async fn create_workspace(
        &self,
        sandbox: &Sandbox,
    ) -> Result<layer2_core::sandbox::Workspace, WorkspaceServiceError> {
        self.workspaces
            .create_for_launch(
                &self.caller,
                Capability::CreateSandbox,
                &sandbox.id,
                CreateWorkspace {
                    profile: self.seed.target.workspace_profile.clone(),
                    confirmed_digest: self.launch.trust.workspace_digest.clone(),
                },
                self.launch.id.to_string(),
            )
            .await
    }

    async fn ready_for_workspace(&mut self) -> Sandbox {
        self.executing(LaunchEffectKind::CreateSandbox).await;
        let sandbox = self.create_sandbox().await.unwrap();
        self.change(|launch, fence, at| {
            launch
                .mark_worker_effect_applied(HOLDER, fence, sandbox.id.to_string(), at)
                .unwrap();
            vec![]
        })
        .await;
        self.change(|launch, fence, at| {
            launch
                .record_worker_sandbox(HOLDER, fence, sandbox.id.clone(), false, at)
                .unwrap()
        })
        .await;
        self.change(|launch, fence, at| {
            launch
                .record_worker_sandbox_ready(HOLDER, fence, None, at)
                .unwrap()
        })
        .await;
        let mut ready = sandbox.clone();
        let active = crate::store::sandbox_maintenance::mongo_integration_tests::active_sandbox(
            sandbox.id.as_str(),
        );
        ready.status = active.status;
        ready.runtime = active.runtime;
        ready.lifecycle = active.lifecycle;
        let mut credential = active.credential;
        credential.source = ready.credential.source.clone();
        credential.reaches = ready.credential.reaches.clone();
        credential.sign_in = ready.credential.sign_in;
        ready.credential = credential;
        ready.work = active.work;
        ready.revision += 1;
        ready.validate().unwrap();
        self.resources
            .sandboxes
            .update_one(
                doc! {"_id":sandbox.id.as_str()},
                doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&ready).unwrap()},
            )
            .await
            .unwrap();
        assert_eq!(
            self.resources.get_sandbox(&sandbox.id).await.unwrap(),
            Some(ready)
        );
        self.executing(LaunchEffectKind::CreateWorkspace).await;
        sandbox
    }

    async fn fence_publisher(&self) {
        let db = self.seed.client.database(&self.seed.database);
        for (collection, filter) in [
            (
                "trigger_definitions",
                doc! {"_id":self.launch.trigger_id.as_str()},
            ),
            (
                "trigger_definition_revisions",
                doc! {"trigger_id":self.launch.trigger_id.as_str()},
            ),
        ] {
            let result = crate::store::durability::majority_collection::<Document>(&db, collection)
                .update_many(
                    filter,
                    doc! {"$set":{"account_erasure_fenced":true},"$currentDate":{"account_erasure_fenced_at":true}},
                )
                .await
                .unwrap();
            assert_eq!(result.matched_count, 1);
        }
        assert!(self
            .seed
            .store
            .trigger_publisher_erased(
                &self.launch.trigger_id,
                &self.launch.revision_digest,
                &self.launch.public_id,
            )
            .await
            .unwrap());
        for name in ["sandbox_profiles", "workspace_profiles", "agent_profiles"] {
            assert_eq!(
                db.collection::<Document>(name)
                    .count_documents(doc! {"account_erasure_fenced":true})
                    .await
                    .unwrap(),
                0
            );
        }
    }
}

async fn declared_agent_under_trigger_workspace(retain_first: bool) {
    use layer2_core::profiles::{AgentProfileSnapshot, AgentProfileStore};
    use layer2_core::sandbox::{
        store::{AgentAdmission, AgentAdmissionStore, WorkspaceStore},
        AgentOrigin, WorkspaceStatus, WorkspaceWorkState, WorkspaceWorkStep,
    };
    let mut fixture = Fixture::new("round3_trigger_declared_agent").await;
    let sandbox = fixture.ready_for_workspace().await;
    let mut workspace = fixture.create_workspace(&sandbox).await.unwrap();
    assert!(matches!(
        workspace.origin,
        layer2_core::sandbox::WorkspaceOrigin::Trigger { .. }
    ));
    let expected = workspace.revision;
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(NOW.into());
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::None, NOW.into());
    workspace.revision += 1;
    fixture
        .resources
        .save_workspace(expected, workspace.clone())
        .await
        .unwrap();
    let db = fixture.seed.client.database(&fixture.seed.database);
    if retain_first {
        erase(&db, &fixture.seed.owner).await;
    } else {
        let mut batch = Batch::default();
        let inventory = ErasureInventory::default();
        assert!(!super::super::barrier::ready(
            &db,
            &[Target {
                subject: &fixture.seed.owner,
                inventory: &inventory
            }],
            &mut Cursor::default(),
            Options {
                agent_shared_observation: Some(false),
                retain_legal_consents: false,
                drain_qualification: Some(TransactionDrainQualification::fixture()),
                billing_enabled: false,
                allocation_rollups_enabled: Some(false),
                transaction_drain_minimum_secs: 120
            },
            &mut batch
        )
        .await
        .unwrap());
    }
    let profile = fixture
        .seed
        .profiles
        .agent_profile_revision(&fixture.seed.target.agent_profile.id, 1)
        .await
        .unwrap()
        .unwrap();
    let admission = AgentAdmission {
        binding: None,
        created_by: None,
        owner: workspace.owner.clone(),
        workspace_id: workspace.id.clone(),
        agent_id: layer2_core::AgentId::generate(),
        worktree_id: layer2_core::WorktreeId::generate(),
        profile: AgentProfileSnapshot::from(&profile),
        capabilities: layer2_core::RuntimeCapabilities::new([
            layer2_core::RuntimeCapability::Conversation,
        ]),
        title: None,
        first_message: None,
        origin: AgentOrigin::WorkspaceProfile { index: 0 },
        creation_key: format!("workspace-profile:{}:0", workspace.id),
        event_at: NOW.into(),
    };
    let mut manual = admission.clone();
    manual.origin = AgentOrigin::Manual;
    manual.creation_key = "new-manual-reference".into();
    assert!(matches!(
        fixture.resources.admit_catalogue_agent(manual).await,
        Err(SandboxStoreError::Conflict)
    ));
    let agents = layer2_core::sandbox::AgentService::new(
        fixture.resources.clone(),
        fixture.seed.profiles.clone(),
        Arc::new(super::super::round3_continuation_tests::NoRuntime),
        layer2_core::RuntimeCapabilities::new([layer2_core::RuntimeCapability::Conversation]),
    );
    assert_eq!(
        agents
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .expect("an existing frozen Trigger Workspace can finish its declared Agent"),
        1
    );
    assert_eq!(
        agents
            .reconcile_profile_agents(&workspace.sandbox_id)
            .await
            .unwrap(),
        0
    );
    db.drop().await.unwrap();
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn frozen_trigger_workspace_finishes_declared_agent_during_profile_erasure() {
    declared_agent_under_trigger_workspace(false).await;
}
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn frozen_trigger_workspace_finishes_declared_agent_after_profile_erasure() {
    declared_agent_under_trigger_workspace(true).await;
}
