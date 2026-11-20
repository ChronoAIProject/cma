//! Real authenticated launch service, retained Mongo sidecar and Agent admission.
use super::*;
use layer2_core::profiles::{AgentProfileSnapshot, AgentProfileStore};
use layer2_core::resource_access::{
    CallerContext, CredentialContext, ResourceAccessStore, ResourceActor,
};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentAdmissionStore, AgentProvenanceStore, AgentStore, WorkspaceStore,
};
use layer2_core::sandbox::{AgentCreationOrigin, AgentOrigin, FixedSandboxCredentialPrerequisite};
use layer2_core::trigger_domain::*;
use layer2_core::{
    Authorization, Caller, Capability, RuntimeCapabilities, RuntimeCapability, WorktreeId,
};
mod common_execution;
mod input_protocol;
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
    identity: CallerContext,
    public: PublicTrigger,
    service: Arc<LaunchService>,
    parent: layer2_core::sandbox::Sandbox,
    workspace: layer2_core::sandbox::Workspace,
}

impl Fixture {
    async fn new(api: bool) -> Self {
        let seed = seeded(if api {
            "initiation_api"
        } else {
            "initiation_human"
        })
        .await;
        let (resources, target) =
            crate::store::sandbox_mongo_integration_tests::resource_access_tests::fixture(
                &seed.client,
                &seed.database,
            )
            .await;
        let resource = resources.resource_access_snapshot(&target).await.unwrap();
        let parent = resource.lineage.sandbox;
        let mut workspace = resource.lineage.workspace.unwrap();
        // The retained-share fixture already owns wt-1. Advance its allocator
        // before exercising normal additional Agent admission.
        let revision = workspace.revision;
        workspace.next_worktree_number = 2;
        workspace.revision += 1;
        resources
            .save_workspace(revision, workspace.clone())
            .await
            .unwrap();
        let resources = Arc::new(resources);
        let mut definition = trigger("trg_initiation", &seed.owner, &seed.target);
        definition.draft.consent.always_review = false;
        definition.draft.reuse.agent = AgentReusePolicy::AlwaysNew;
        seed.store.create_trigger(definition).await.unwrap();
        let public = seed
            .store
            .publish_trigger(
                &TriggerId("trg_initiation".into()),
                &seed.owner,
                1,
                NOW.into(),
            )
            .await
            .unwrap();
        let caller = Caller::new(
            parent.owner.clone(),
            Authorization::new([Capability::CreateSandbox, Capability::LaunchTriggers]),
        );
        let identity = CallerContext {
            credential_source: layer2_core::resource_access::CredentialSource::Retained,
            actor: if api {
                ResourceActor::Api {
                    subject: caller.subject.clone(),
                    delegation_id: None,
                }
            } else {
                ResourceActor::Human {
                    subject: caller.subject.clone(),
                }
            },
            credential: CredentialContext {
                issuer: "https://nyx.example".into(),
                subject: caller.subject.clone(),
                credential_id: "actual-launch-request".into(),
            },
            caller,
            memberships: vec![],
        };
        let launches = Arc::new(seed.store.clone());
        let service = LaunchService::new(
            launches.clone(),
            seed.profiles.clone(),
            launches,
            resources.clone(),
            Arc::new(Connected),
            Arc::new(LaunchPolicy {
                enabled: true,
                orchestration_paused: false,
                visitor_launches_per_day: 5,
            }),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        );
        Self {
            seed,
            resources,
            identity,
            public,
            service,
            parent,
            workspace,
        }
    }
    fn request(&self, key: &str) -> CreateOrResumeLaunch {
        CreateOrResumeLaunch {
            placement_id: None,
            referrer_origin: None,
            start_if_ready: true,
            choice: None,
            idempotency_key: key.into(),
        }
    }
    async fn launch(&self, key: &str) -> Launch {
        self.service
            .create_or_resume_as(&self.identity, &self.public.public_id, self.request(key))
            .await
            .unwrap()
            .launch
    }
    async fn step(
        &self,
        launch: Launch,
        mutation: impl FnOnce(&mut Launch, u64, String) -> Vec<LaunchEventKind>,
    ) -> Launch {
        let now = layer2_core::now_rfc3339();
        let mut claimed = self
            .seed
            .store
            .claim_launch(
                &launch.id,
                "initiator-worker",
                now.clone(),
                layer2_core::rfc3339_at(
                    std::time::SystemTime::now() + std::time::Duration::from_secs(60),
                ),
            )
            .await
            .unwrap()
            .unwrap();
        let expected_revision = claimed.revision;
        let fence = claimed.work.lease.as_ref().unwrap().fence;
        let events = mutation(&mut claimed, fence, now.clone());
        self.seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision,
                launch: claimed,
                event_at: now,
                events,
            })
            .await
            .unwrap()
    }
    async fn ready_for_agent(&self, launch: Launch) -> Launch {
        let launch = self
            .step(launch, |launch, fence, now| {
                launch
                    .record_worker_sandbox(
                        "initiator-worker",
                        fence,
                        self.parent.id.clone(),
                        true,
                        now,
                    )
                    .unwrap()
            })
            .await;
        let launch = self
            .step(launch, |launch, fence, now| {
                launch
                    .record_worker_sandbox_ready(
                        "initiator-worker",
                        fence,
                        Some(self.workspace.id.clone()),
                        now,
                    )
                    .unwrap()
            })
            .await;
        let launch = self
            .step(launch, |launch, fence, now| {
                launch
                    .record_worker_workspace(
                        "initiator-worker",
                        fence,
                        self.workspace.id.clone(),
                        true,
                        now,
                    )
                    .unwrap()
            })
            .await;
        let launch = self
            .step(launch, |launch, fence, now| {
                launch
                    .record_worker_workspace_ready("initiator-worker", fence, now)
                    .unwrap()
            })
            .await;
        let launch = self
            .step(launch, |launch, fence, now| {
                launch
                    .prepare_worker_effect(
                        "initiator-worker",
                        fence,
                        LaunchEffectKind::CreateAgent,
                        now,
                    )
                    .unwrap()
            })
            .await;
        // The real worker retains its current claim across prepared/executing.
        let mut launch = launch;
        let expected_revision = launch.revision;
        let fence = launch.work.lease.as_ref().unwrap().fence;
        let now = layer2_core::now_rfc3339();
        launch
            .mark_worker_effect_executing("initiator-worker", fence, now.clone())
            .unwrap();
        self.seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision,
                launch,
                event_at: now,
                events: vec![],
            })
            .await
            .unwrap()
    }
    async fn admission(&self, launch: &Launch) -> AgentAdmission {
        let profile = self
            .seed
            .profiles
            .agent_profile_revision(
                &self.seed.target.agent_profile.id,
                self.seed.target.agent_profile.revision,
            )
            .await
            .unwrap()
            .unwrap();
        let origin = AgentOrigin::Trigger {
            trigger_id: launch.trigger_id.to_string(),
            launch_id: launch.id.to_string(),
        };
        AgentAdmission {
            binding: None,
            created_by: Some(ResourceActor::Trigger {
                subject: launch.owner.clone(),
                launch_id: launch.id.clone(),
            }),
            owner: launch.owner.clone(),
            workspace_id: self.workspace.id.clone(),
            agent_id: layer2_core::AgentId::generate(),
            worktree_id: WorktreeId::generate(),
            profile: AgentProfileSnapshot::from(&profile),
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            title: None,
            first_message: None,
            creation_key: format!(
                "trigger:{}",
                layer2_core::profiles::content_digest(&origin).unwrap()
            ),
            origin,
            event_at: layer2_core::now_rfc3339(),
        }
    }
    async fn close(self) {
        self.seed
            .client
            .database(&self.seed.database)
            .drop()
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn trigger_initiation_service_admission_is_atomic_and_replay_reopen_never_relabels_actor() {
    let f = Fixture::new(true).await;
    f.seed.client.database(&f.seed.database).run_command(doc! {"collMod":LAUNCH_EVENTS,"validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert!(f
        .service
        .create_or_resume_as(&f.identity, &f.public.public_id, f.request("atomic-launch"))
        .await
        .is_err());
    assert_eq!(
        f.seed
            .client
            .database(&f.seed.database)
            .collection::<Document>(LAUNCHES)
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.seed
        .client
        .database(&f.seed.database)
        .run_command(doc! {"collMod":LAUNCH_EVENTS,"validator":{}})
        .await
        .unwrap();
    let launch = f.launch("atomic-launch").await;
    let mut other_actor = f.identity.clone();
    other_actor.actor = ResourceActor::Human {
        subject: launch.owner.clone(),
    };
    assert_eq!(
        f.service
            .create_or_resume_as(
                &other_actor,
                &f.public.public_id,
                f.request("atomic-launch")
            )
            .await
            .unwrap()
            .launch,
        launch
    );
    let reopened = MongoTriggerDefinitionStore::open(
        &f.seed.client,
        &f.seed.database,
        f.seed.profiles.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap(),
        launch
    );
    let document = f
        .seed
        .client
        .database(&f.seed.database)
        .collection::<Document>(LAUNCHES)
        .find_one(doc! {"_id":launch.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert!(document.contains_key("launch_initiation"));
    assert!(!document
        .get_document("record")
        .unwrap()
        .contains_key("initiation"));
    let advanced = f.ready_for_agent(launch.clone()).await;
    assert_eq!(advanced.initiation, launch.initiation);
    let mut changed = advanced.clone();
    changed.initiation = None;
    changed.revision += 1;
    assert!(reopened
        .commit_launch(LaunchCommit {
            expected_revision: advanced.revision,
            launch: changed,
            event_at: advanced.updated_at.clone(),
            events: vec![]
        })
        .await
        .is_err());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn trigger_initiation_agent_creation_is_atomic_with_original_human_or_api_actor() {
    for api in [false, true] {
        let f = Fixture::new(api).await;
        let launch = f.ready_for_agent(f.launch("create-agent").await).await;
        let initiation = launch.initiation.clone().unwrap();
        let request = f.admission(&launch).await;
        f.seed
            .client
            .database(&f.seed.database)
            .create_collection("agent_creation_provenance")
            .await
            .unwrap();
        f.seed.client.database(&f.seed.database).run_command(doc! {"collMod":"agent_creation_provenance","validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
        assert!(f
            .resources
            .admit_agent_with_trigger_initiator(request.clone(), initiation.clone(), false)
            .await
            .is_err());
        assert!(f
            .resources
            .get_agent(&request.agent_id)
            .await
            .unwrap()
            .is_none());
        f.seed
            .client
            .database(&f.seed.database)
            .run_command(doc! {"collMod":"agent_creation_provenance","validator":{}})
            .await
            .unwrap();
        let creation = f
            .resources
            .admit_agent_with_trigger_initiator(request.clone(), initiation.clone(), false)
            .await
            .unwrap();
        let agent = &creation.creation().agent;
        let before = f
            .resources
            .agent_provenance_snapshot(&agent.id)
            .await
            .unwrap()
            .creation_provenance(&agent.id)
            .unwrap();
        assert_eq!(before.created_by, request.created_by);
        assert!(
            matches!(&before.creation_origin, AgentCreationOrigin::Trigger { launching_actor: Some(actor), .. } if actor == &f.identity.actor)
        );
        assert_eq!(
            f.resources
                .admit_agent_with_trigger_initiator(request, initiation, false)
                .await
                .unwrap()
                .creation()
                .agent,
            *agent
        );
        assert_eq!(
            f.resources
                .agent_provenance_snapshot(&agent.id)
                .await
                .unwrap()
                .creation_provenance(&agent.id)
                .unwrap(),
            before
        );
        let foreign = f.launch("other-launch").await;
        let mut substituted = f.admission(&launch).await;
        substituted.creation_key.push_str("-substituted");
        assert!(f
            .resources
            .admit_agent_with_trigger_initiator(
                substituted.clone(),
                foreign.initiation.unwrap(),
                false
            )
            .await
            .is_err());
        assert!(f
            .resources
            .get_agent(&substituted.agent_id)
            .await
            .unwrap()
            .is_none());
        f.close().await;
    }
}
