use super::super::{fixture::RenewalFixture, *};
use crate::{
    bridge::provisioning::revival::{ChannelRevivalAuthority, ChannelRevivalAuthoritySource},
    sandbox::{
        channel_credentials::ChannelGuardedCredentials,
        channel_lifecycle_credentials::ChannelLifecycleCredentialDriver,
        worker::{
            CheckpointObjectPurger, LiveAgentStopper, ProfileWorkspaceProvisioner,
            SandboxDependencyError, SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies,
        },
        LiveAgentRegistry, ManagedSandboxCredentials, OwnerTokenSource,
    },
};
use layer2_core::{
    bridge::{
        allocation::{AllocationAuthority, AllocationRecord},
        BridgeError,
    },
    sandbox::{
        store::{SandboxLifecycleCommand, SandboxLifecycleCommandStore, SandboxStore},
        AgentOperationService, DurableAgentUpdateRecorder, Sandbox, SandboxPolicy,
        SandboxReviveCause,
    },
    SubjectId,
};
use layer3_adapter::RuntimeHandle;
use std::sync::Arc;

struct NoOwner;
#[async_trait::async_trait]
impl OwnerTokenSource for NoOwner {
    async fn token_for(
        &self,
        _: &SubjectId,
    ) -> Result<Option<crate::auth::persistence::SecretValue>, String> {
        panic!("Channel revival entered owner token lane")
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
        panic!("revival recreated profile children")
    }
}
#[async_trait::async_trait]
impl LiveAgentStopper for NoEffects {
    async fn stop_live_agents(
        &self,
        _: &Sandbox,
        _: Option<&RuntimeHandle>,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("revival entered retirement")
    }
}
#[async_trait::async_trait]
impl CheckpointObjectPurger for NoEffects {
    async fn purge_checkpoint_objects(
        &self,
        _: &Sandbox,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("revival purged checkpoints")
    }
}
struct Authority {
    allocation: AllocationRecord,
    authority: AllocationAuthority,
    human: Arc<crate::bridge::BridgeControlPlaneLease>,
}
#[async_trait::async_trait]
impl ChannelRevivalAuthoritySource for Authority {
    async fn resolve(&self, sandbox: &Sandbox) -> Result<ChannelRevivalAuthority, BridgeError> {
        Ok(ChannelRevivalAuthority::fixture(
            self.allocation.clone(),
            self.authority.clone(),
            self.human.clone(),
            sandbox.clone(),
        ))
    }
}
pub(crate) struct LifecycleFixture {
    pub f: RenewalFixture,
    pub worker: Arc<SandboxWorker>,
    pub registry: Arc<LiveAgentRegistry>,
    pub active: Sandbox,
}
impl LifecycleFixture {
    pub async fn new() -> Self {
        let f = RenewalFixture::new().await;
        let target = f.original.record.placement.target();
        let registry = crate::sandbox::channel_startup::test_support::publish_connection(
            f.original.resources.clone(),
            f.original.adapter.clone(),
            &target.agent_id,
        )
        .await;
        let active = f
            .original
            .resources
            .get_sandbox(&target.sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let driver = Arc::new(ChannelLifecycleCredentialDriver {
            resources: f.original.resources.clone(),
            authority: Arc::new(Authority {
                allocation: f.original.record.clone(),
                authority: f.original.authority.clone(),
                human: f.original.lease.clone(),
            }),
            allocations: f.original.allocations.clone(),
            credentials: f.driver.credentials.clone(),
            issuer: f.driver.issuer.clone(),
            custody: f.original.custody.clone(),
            adapter: f.original.adapter.clone(),
        });
        let ordinary = ManagedSandboxCredentials::new(
            f.driver.issuer.clone(),
            Arc::new(NoOwner),
            f.original.custody.clone(),
        );
        let credentials = ChannelGuardedCredentials::with_lifecycle(
            ordinary,
            f.original.allocations.clone(),
            Some(driver),
        );
        let worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: f.original.resources.clone(),
                adapter: f.original.adapter.clone(),
                credentials,
                workspaces: Arc::new(NoEffects),
                agents: Arc::new(NoEffects),
                checkpoints: Arc::new(NoEffects),
                registry: registry.clone(),
                updates: DurableAgentUpdateRecorder::new(f.original.resources.clone()),
                operations: AgentOperationService::new(
                    f.original.resources.clone(),
                    registry.clone(),
                ),
            },
            SandboxWorkerConfig::new(
                "channel-revival-worker",
                "fixture-deployment",
                f.original.settings.runtime_version.clone(),
                SandboxPolicy::default(),
            ),
        );
        Self {
            f,
            worker,
            registry,
            active,
        }
    }
    pub async fn sandbox(&self) -> Sandbox {
        self.f
            .original
            .resources
            .get_sandbox(&self.active.id)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn step(&self) {
        assert!(self
            .worker
            .run_one(&self.active.id, SystemTime::now())
            .await
            .unwrap());
    }
    pub async fn release_for_retry(&self) {
        let mut current = self.sandbox().await;
        let previous = current.revision;
        current.work.lease = None;
        current.work.ready_at = layer2_core::now_rfc3339();
        current.revision += 1;
        let fields = crate::store::sandboxes::sandbox_replacement_fields(&current).unwrap();
        self.f
            .original
            .client
            .database(&self.f.original.database)
            .collection::<Document>("sandboxes")
            .update_one(
                doc! {"_id":current.id.as_str(),"revision":previous as i64},
                doc! {"$set":fields},
            )
            .await
            .unwrap();
    }
    pub async fn expire_paused_key(&self) {
        let mut sandbox = self.sandbox().await;
        assert_eq!(sandbox.status, SandboxStatus::Paused);
        assert_eq!(sandbox.credential.key_generation, 1);
        let mut material = self
            .f
            .original
            .custody
            .get(&sandbox.id, 1)
            .await
            .unwrap()
            .unwrap();
        let now = SystemTime::now();
        material.authority.issued_at = layer2_core::rfc3339_at(now - Duration::from_secs(3600));
        material.authority.expires_at = layer2_core::rfc3339_at(now - Duration::from_secs(1));
        self.f
            .state
            .keys
            .lock()
            .unwrap()
            .get_mut(&material.authority.provider_key_id)
            .unwrap()["expires_at"] = serde_json::json!(material.authority.expires_at);
        self.f
            .original
            .custody
            .delete(&sandbox.id, 1)
            .await
            .unwrap();
        self.f
            .original
            .custody
            .insert(material.clone())
            .await
            .unwrap();
        sandbox.credential.current = Some(material.authority);
        sandbox.revision += 1;
        let fields = crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap();
        self.f
            .original
            .client
            .database(&self.f.original.database)
            .collection::<Document>("sandboxes")
            .update_one(doc! {"_id":sandbox.id.as_str()}, doc! {"$set":fields})
            .await
            .unwrap();
    }
    pub async fn pause(&self) -> Sandbox {
        self.f
            .original
            .resources
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::pause(
                self.active.id.clone(),
                self.active.owner.clone(),
                layer2_core::now_rfc3339(),
                None,
            ))
            .await
            .unwrap();
        for _ in 0..4 {
            self.step().await;
        }
        let paused = self.sandbox().await;
        assert_eq!(paused.status, SandboxStatus::Paused);
        paused
    }
    pub async fn revive(&self) {
        self.f
            .original
            .resources
            .schedule_sandbox_lifecycle(SandboxLifecycleCommand::revive(
                self.active.id.clone(),
                self.active.owner.clone(),
                layer2_core::now_rfc3339(),
                SandboxReviveCause::ExplicitRequest,
            ))
            .await
            .unwrap();
    }
    pub async fn to_delivery(&self) {
        self.pause().await;
        self.revive().await;
        for _ in 0..3 {
            self.step().await;
        }
        assert!(matches!(
            self.sandbox().await.lifecycle.operation,
            Some(layer2_core::sandbox::SandboxLifecycleOperation::Revive {
                phase: layer2_core::sandbox::SandboxRevivePhase::CredentialDelivery,
                ..
            })
        ));
    }
    pub async fn close(self) {
        drop(self.registry);
        self.f.close().await;
    }
}
