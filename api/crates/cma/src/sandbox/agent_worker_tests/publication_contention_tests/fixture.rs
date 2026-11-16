use super::*;
use layer2_core::profiles::WorkspaceProfileRef;
use layer2_core::sandbox::store::{SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore};
use layer2_core::{Authorization, Caller, Capability};

pub(super) fn inject(
    h: &mut Harness,
    skills: Arc<dyn AgentSkillResolver>,
    boundary: Boundary,
    fault: Fault,
) -> Arc<PublicationStore> {
    let store = Arc::new(PublicationStore {
        inner: h.store.clone(),
        attempts: AtomicUsize::new(0),
        fault,
        boundary,
    });
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.lease_duration = Duration::from_secs(30);
    config.retry_delay = Duration::from_secs(1);
    config.open_timeout = Duration::from_secs(2);
    h.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: h.adapter.clone(),
            skills,
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(h.store.clone(), h.registry.clone()),
        },
        config,
    );
    store
}

pub(super) async fn claim(h: &Harness, id: &AgentId) -> Agent {
    h.store
        .claim_agent_work(
            id,
            "agent-worker",
            rfc3339_at(h.now),
            rfc3339_at(h.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn calls(h: &Harness, operation: WireOperation) -> usize {
    h.backend
        .requests()
        .await
        .iter()
        .filter(|(_, request)| request.operation() == operation)
        .count()
}

pub(super) async fn skill_fixture() -> (Harness, Arc<ScriptedSkills>, Agent) {
    let material = super::super::skill_material_tests::runtime_skill("7.0", 'a', "archive");
    let skills = ScriptedSkills::new([Ok(vec![]), Ok(vec![material])]);
    let mut h = Harness::new(skills.clone()).await;
    h.now = SystemTime::now();
    let admitted = h.admit("publication-skill", vec![]).await;
    let ready = h.drive_ready(&admitted.id).await;
    AgentOperationService::new(h.store.clone(), h.registry.clone())
        .install_skill(
            &Caller::new(
                ready.owner.clone(),
                Authorization::new([Capability::UseOwnSandboxes]),
            ),
            &ready.id,
            SkillRef {
                source: SkillSource::Ornn,
                id: SkillId::new("research"),
                version: Some("7.0".into()),
            },
        )
        .await
        .unwrap();
    h.now = SystemTime::now();
    let claimed = claim(&h, &ready.id).await;
    (h, skills, claimed)
}

/// Real lifecycle admission reserves an unbound Worktree while parents prepare.
/// No shared bootstrap fixture or admission invariants are changed for this suite.
pub(super) struct Waiting {
    pub h: Harness,
    ready_parent: Sandbox,
    ready_workspace: Workspace,
}

impl Waiting {
    pub async fn new() -> Self {
        let original = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let mut ready_parent = original
            .store
            .get_sandbox(&original.workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        ready_parent.origin = SandboxOrigin::Lifecycle {
            operation_id: "publication-wait".into(),
        };
        ready_parent.profile.content.workspace_profiles = vec![WorkspaceProfileRef::new(
            original.workspace.profile.id.clone(),
            original.workspace.profile.revision,
        )];
        ready_parent.profile.digest =
            layer2_core::profiles::content_digest(&ready_parent.profile.content).unwrap();
        let mut ready_workspace = original.workspace.clone();
        ready_workspace.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
        let mut parent = ready_parent.clone();
        parent.status = SandboxStatus::Provisioning;
        parent.runtime = None;
        parent.credential.current = None;
        parent.credential.delivered = false;
        parent.lifecycle.runtime_generation_high_water = 0;
        parent.work =
            SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, rfc3339_at(original.now));
        let store = Arc::new(InMemorySandboxStore::default());
        store
            .admit_sandbox(
                SandboxAdmission::new(
                    parent.clone(),
                    SandboxAdmissionName::Exact(parent.name.clone()),
                    4,
                    rfc3339_at(original.now),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut workspace = ready_workspace.clone();
        workspace.status = WorkspaceStatus::Preparing;
        workspace.resolved_commit = None;
        workspace.ready_at = None;
        workspace.work =
            WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, rfc3339_at(original.now));
        store.create_workspace(workspace.clone()).await.unwrap();
        let registry = Arc::new(LiveAgentRegistry::default());
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter: original.adapter.clone(),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: registry.clone(),
                updates: updates.clone(),
                operations: AgentOperationService::new(store.clone(), registry.clone()),
            },
            AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID),
        );
        Self {
            h: Harness {
                store,
                worker,
                registry,
                backend: original.backend,
                adapter: original.adapter,
                runtime: original.runtime,
                workspace,
                now: SystemTime::now(),
                updates,
            },
            ready_parent,
            ready_workspace,
        }
    }

    pub async fn admit(&self) -> Agent {
        let h = &self.h;
        let origin = AgentOrigin::Lifecycle {
            operation_id: "publication-wait".into(),
        };
        let key = format!(
            "lifecycle:{}",
            layer2_core::profiles::content_digest(&origin).unwrap()
        );
        let admitted = h
            .store
            .admit_agent(AgentAdmission {
                binding: None,
                created_by: None,
                owner: h.workspace.owner.clone(),
                workspace_id: h.workspace.id.clone(),
                agent_id: AgentId::generate(),
                worktree_id: WorktreeId::generate(),
                profile: agent_profile(vec![]),
                capabilities: RuntimeCapabilities::new([
                    RuntimeCapability::Conversation,
                    RuntimeCapability::Steer,
                    RuntimeCapability::Interrupt,
                    RuntimeCapability::Usage,
                    RuntimeCapability::Resources,
                ]),
                title: None,
                first_message: None,
                origin,
                creation_key: key,
                event_at: rfc3339_at(h.now),
            })
            .await
            .unwrap();
        let agent = admitted.creation().agent.clone();
        assert_eq!(agent.work.step, AgentWorkStep::WaitForWorkspace);
        agent
    }

    pub async fn ready(&self) {
        let h = &self.h;
        let current = h
            .store
            .get_workspace(&h.workspace.id)
            .await
            .unwrap()
            .unwrap();
        let mut workspace = self.ready_workspace.clone();
        workspace.next_worktree_number = current.next_worktree_number;
        workspace.revision = current.revision + 1;
        h.store
            .save_workspace(current.revision, workspace)
            .await
            .unwrap();
        let current = h
            .store
            .get_sandbox(&h.workspace.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let mut parent = self.ready_parent.clone();
        parent.revision = current.revision + 1;
        h.store
            .save_sandbox(current.revision, parent)
            .await
            .unwrap();
    }
}
