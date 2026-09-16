use super::*;
use crate::{
    bridge::provisioning::retirement::{
        ChannelRetirementAuthority, ChannelRetirementAuthoritySource,
    },
    sandbox::{
        channel_credentials::ChannelGuardedCredentials,
        channel_retirement::ChannelRetirementDriver,
        worker::{
            CheckpointObjectPurger, LiveAgentStopper, ProfileWorkspaceProvisioner,
            SandboxDependencyError, SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies,
        },
        LiveAgentRegistry, ManagedSandboxCredentials, OwnerTokenSource,
    },
    store::MongoSandboxStore,
};
use layer2_core::{
    bridge::{
        allocation::{AllocationAuthority, AllocationRecord},
        BridgeError,
    },
    sandbox::{
        store::{AgentStore, SandboxLifecycleExecutionStore, SandboxStore},
        AgentOperationService, AgentStatus, AgentStopReason, DurableAgentUpdateRecorder, Sandbox,
        SandboxPolicy, SandboxRetirement, StoredSandboxRetirement,
    },
    SubjectId,
};
use layer3_adapter::RuntimeHandle;
use std::sync::{atomic::AtomicUsize, Arc};
#[path = "retirement_adapter.rs"]
mod adapter;
pub(crate) use adapter::RetirementAdapter;

struct NoOwner;
#[async_trait::async_trait]
impl OwnerTokenSource for NoOwner {
    async fn token_for(
        &self,
        _: &SubjectId,
    ) -> Result<Option<crate::auth::persistence::SecretValue>, String> {
        panic!("Channel retirement used an owner token")
    }
}
struct NoEffects;
#[async_trait::async_trait]
impl ProfileWorkspaceProvisioner for NoEffects {
    async fn create_profile_workspace(
        &self,
        _: &Sandbox,
        _: &RuntimeHandle,
        _: &layer2_core::profiles::WorkspaceProfileRef,
        _: u16,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("retirement created a workspace")
    }
}
#[async_trait::async_trait]
impl CheckpointObjectPurger for NoEffects {
    async fn purge_checkpoint_objects(
        &self,
        _: &Sandbox,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("Channel retirement entered unimplemented retention cleanup")
    }
}
pub(crate) struct StopRecorder {
    pub calls: AtomicUsize,
    resources: Arc<MongoSandboxStore>,
}
#[async_trait::async_trait]
impl LiveAgentStopper for StopRecorder {
    async fn stop_live_agents(
        &self,
        sandbox: &Sandbox,
        _: Option<&RuntimeHandle>,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let rows = self
            .resources
            .sandbox_agents(
                &sandbox.id,
                &layer2_core::sandbox::store::PageRequest::new(50, None).unwrap(),
            )
            .await
            .unwrap();
        assert!(rows.next.is_none());
        for mut agent in rows.items {
            if agent.status.is_live() {
                let revision = agent.revision;
                agent
                    .transition(
                        AgentStatus::Stopped,
                        layer2_core::now_rfc3339(),
                        Some(AgentStopReason::SandboxRetired),
                    )
                    .unwrap();
                self.resources.save_agent(revision, agent).await.unwrap();
            }
        }
        Ok(())
    }
}
struct Authority {
    allocation: AllocationRecord,
    authority: AllocationAuthority,
    human: Arc<crate::bridge::BridgeControlPlaneLease>,
}
#[async_trait::async_trait]
impl ChannelRetirementAuthoritySource for Authority {
    async fn resolve(&self, sandbox: &Sandbox) -> Result<ChannelRetirementAuthority, BridgeError> {
        Ok(ChannelRetirementAuthority::fixture(
            self.allocation.clone(),
            self.authority.clone(),
            self.human.clone(),
            sandbox.clone(),
        ))
    }
}
pub(crate) struct RetirementFixture {
    pub f: RenewalFixture,
    pub worker: Arc<SandboxWorker>,
    pub stops: Arc<StopRecorder>,
    pub adapter: Arc<RetirementAdapter>,
}
impl RetirementFixture {
    pub async fn new(renewal_steps: usize) -> Self {
        let f = RenewalFixture::new().await;
        for _ in 0..renewal_steps {
            f.step().await.unwrap();
        }
        Self::from_renewal(f)
    }
    pub fn from_renewal(f: RenewalFixture) -> Self {
        Self::with_agents(f, Arc::new(LiveAgentRegistry::default()), None)
    }
    pub fn with_agents(
        f: RenewalFixture,
        registry: Arc<LiveAgentRegistry>,
        agents: Option<Arc<dyn LiveAgentStopper>>,
    ) -> Self {
        let stops = Arc::new(StopRecorder {
            calls: AtomicUsize::new(0),
            resources: f.original.resources.clone(),
        });
        let adapter = Arc::new(RetirementAdapter::new(f.original.adapter.clone()));
        let driver = Arc::new(ChannelRetirementDriver {
            authority: Arc::new(Authority {
                allocation: f.original.record.clone(),
                authority: f.original.authority.clone(),
                human: f.original.lease.clone(),
            }),
            allocations: f.original.allocations.clone(),
            credentials: f.driver.credentials.clone(),
            issuer: f.driver.issuer.clone(),
            custody: f.original.custody.clone(),
            adapter: adapter.clone(),
        });
        let ordinary = ManagedSandboxCredentials::new(
            f.driver.issuer.clone(),
            Arc::new(NoOwner),
            f.original.custody.clone(),
        );
        let credentials = ChannelGuardedCredentials::with_retirement(
            ordinary,
            f.original.allocations.clone(),
            None,
            Some(driver),
        );
        let worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: f.original.resources.clone(),
                adapter: adapter.clone(),
                credentials,
                workspaces: Arc::new(NoEffects),
                agents: agents.unwrap_or_else(|| stops.clone()),
                checkpoints: Arc::new(NoEffects),
                registry: registry.clone(),
                updates: DurableAgentUpdateRecorder::new(f.original.resources.clone()),
                operations: AgentOperationService::new(f.original.resources.clone(), registry),
            },
            SandboxWorkerConfig::new(
                "channel-retirement-worker",
                "fixture-deployment",
                f.original.settings.runtime_version.clone(),
                SandboxPolicy::default(),
            ),
        );
        Self {
            f,
            worker,
            stops,
            adapter,
        }
    }
    pub async fn start(&self) {
        StoredSandboxRetirement::new(self.f.original.resources.clone())
            .retire(
                &self.f.original.record.placement.target().sandbox_id,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
    }
    pub async fn sandbox(&self) -> Sandbox {
        self.f
            .original
            .resources
            .get_sandbox(&self.f.original.record.placement.target().sandbox_id)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn step(&self) {
        assert!(self
            .worker
            .run_one(
                &self.f.original.record.placement.target().sandbox_id,
                SystemTime::now()
            )
            .await
            .unwrap());
    }
    pub async fn retry(&self) {
        let mut sandbox = self.sandbox().await;
        let revision = sandbox.revision;
        sandbox.work.lease = None;
        sandbox.work.ready_at = layer2_core::now_rfc3339();
        sandbox.revision += 1;
        let fields = crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap();
        self.db()
            .collection::<Document>("sandboxes")
            .update_one(
                doc! {"_id":sandbox.id.as_str(),"revision":revision as i64},
                doc! {"$set":fields},
            )
            .await
            .unwrap();
    }
    pub fn db(&self) -> mongodb::Database {
        self.f.original.client.database(&self.f.original.database)
    }
    pub async fn receipt(
        &self,
    ) -> Option<crate::sandbox::channel_retirement::model::ChannelRetirementState> {
        let row = self
            .db()
            .collection::<Document>("sandboxes")
            .find_one(doc! {"_id":self.f.original.record.placement.target().sandbox_id.as_str()})
            .await
            .unwrap()
            .unwrap();
        row.get("channel_retirement")
            .cloned()
            .map(|value| mongodb::bson::deserialize_from_bson(value).unwrap())
    }
    pub async fn close(self) {
        self.f.close().await;
    }
}
