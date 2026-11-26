use super::*;
use crate::sandbox::{
    seed_sandbox_profile_with_capacity,
    worker::{SandboxWorker, SandboxWorkerConfig, SandboxWorkerDependencies},
    ActionLog, AgentStopRecorder, LiveAgentRegistry, MemoryCredentials, WorkspaceRecorder,
};
use crate::store::{open_pointer_fixture_sandbox_store, MongoProfileStore, MongoSandboxStore};
use layer2_core::profiles::SandboxProfileRef;
use layer2_core::sandbox::{
    store::{
        SandboxImageRotationRequest, SandboxMaintenanceOutcome, SandboxMaintenanceStore,
        SandboxStore,
    },
    AgentOperationService, CreateSandbox, DurableAgentUpdateRecorder,
    FixedSandboxCredentialPrerequisite, SandboxOrigin, SandboxPolicy, SandboxRuntimeUpdateService,
    SandboxService, SandboxStatus,
};
use layer2_core::{Authorization, Caller, Capability, SandboxProfileId, SubjectId};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    hosted::HostedDeployment,
};
use std::time::SystemTime;

use crate::warm_pools::tests::runtime_pointer::lifecycle_backend::{Backend, SnapshotFixture};

pub(in super::super) struct Lifecycle {
    store: Arc<MongoSandboxStore>,
    released: Arc<StoreReleasedRuntime>,
    service: Arc<SandboxService>,
    profiles: Arc<MongoProfileStore>,
    updates: SandboxRuntimeUpdateService,
    worker: Arc<SandboxWorker>,
    profile: SandboxProfileRef,
}

impl ControllerReplica<'_> {
    pub(in super::super) async fn lifecycle(&self) -> TestResult<&Lifecycle> {
        self.lifecycle
            .get_or_try_init(|| {
                Lifecycle::new(
                    self.replica,
                    self.controller.release.clone(),
                    image(Lane::Agent, 'a'),
                    image(Lane::Machine, '1'),
                )
            })
            .await
    }
}

impl Lifecycle {
    pub(super) async fn new(
        replica: &Replica,
        released: Arc<StoreReleasedRuntime>,
        pin: String,
        machine_pin: String,
    ) -> TestResult<Self> {
        let store = Arc::new(
            open_pointer_fixture_sandbox_store(&replica.database, pin.clone(), released.clone())
                .await?,
        );
        let profiles = Arc::new(
            MongoProfileStore::open(replica.database.client(), replica.database.name()).await?,
        );
        let profile = seed_sandbox_profile_with_capacity(
            profiles.as_ref(),
            SandboxProfileId(format!("sbp_{}", replica.mongo_app_name)),
            vec![],
            vec![],
            capacity(),
            None,
        )
        .await;
        let policy = SandboxPolicy {
            max_sandboxes_per_owner: 10,
            max_sandboxes_per_visitor: 1,
            inactivity_pause_after_seconds: 900,
            checkpoint_interval_seconds: layer2_core::DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
            retention_grace: Duration::from_secs(60),
            baseline_services: vec![],
        };
        let service = SandboxService::new(
            store.clone(),
            profiles.clone(),
            policy.clone(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        );
        let local_home = layer3_adapter::HomeVolumePolicy {
            mode: layer3_adapter::HomeVolumeMode::LegacySnapshot,
            template_name: "fixture-snapshot".into(),
            template_generation: "initial".into(),
            ..home_policy()
        };
        let homes = Arc::new(layer3_adapter::RoutedHomeVolumes::new(
            Some((Arc::new(SnapshotFixture), local_home.clone())),
            Some((Arc::new(replica.provider.clone()), home_policy())),
        )?);
        let adapter = Arc::new(
            CodexAdapter::with_backend(
                Backend {
                    provider: replica.provider.clone(),
                    hosted: replica.hosted.clone(),
                    homes: homes.clone(),
                },
                CodexSandboxSpec {
                    deployment: HostedDeployment {
                        image: deployment_image(pin.clone()),
                        ..HostedDeployment::default()
                    },
                    ..CodexSandboxSpec::default()
                },
            )
            .with_native_runtime_image(deployment_image(machine_pin))?
            .with_home_volumes(homes),
        );
        let actions = ActionLog::default();
        let registry = Arc::new(LiveAgentRegistry::default());
        let mut config = SandboxWorkerConfig::new(&replica.mongo_app_name, DEPLOYMENT, pin, policy)
            .with_home_policy(Some(local_home))
            .with_released_runtime(released.clone());
        config.retry_delay = Duration::ZERO;
        let worker = SandboxWorker::new(
            SandboxWorkerDependencies {
                store: store.clone(),
                adapter,
                credentials: MemoryCredentials::new(actions.clone()),
                workspaces: Arc::new(WorkspaceRecorder {
                    actions: actions.clone(),
                }),
                agents: Arc::new(AgentStopRecorder { actions }),
                registry: registry.clone(),
                updates: DurableAgentUpdateRecorder::new(store.clone()),
                operations: AgentOperationService::new(store.clone(), registry),
            },
            config,
        );
        Ok(Self {
            updates: SandboxRuntimeUpdateService::with_released(store.clone(), released.clone()),
            released,
            store,
            service,
            profiles,
            worker,
            profile,
        })
    }

    pub(super) async fn admit(&self, lane: Lane, key: &str) -> TestResult<Sandbox> {
        if lane == Lane::Machine {
            let release = self.released.refresh().await?;
            return crate::store::native_prepared::admit_machine_for_pointer_fixture(
                &self.store,
                &release,
                owner().subject.as_str(),
                key,
            )
            .await;
        }
        let sandbox = self.create_agent_sandbox(key).await?;
        self.drive_active(&sandbox.id).await
    }

    pub(in super::super) async fn create_agent_sandbox(&self, key: &str) -> TestResult<Sandbox> {
        Ok(self
            .service
            .create(
                &owner(),
                CreateSandbox {
                    profile: self.profile.clone(),
                    name: Some(key.into()),
                    confirmed_digest: None,
                    origin: SandboxOrigin::Manual,
                },
            )
            .await?)
    }

    pub(super) async fn sandbox(&self, id: &SandboxId) -> TestResult<Sandbox> {
        self.store
            .get_sandbox(id)
            .await?
            .ok_or_else(|| format!("fixture Sandbox {id} is missing").into())
    }

    pub(super) async fn create_pinned_lifecycle_sandbox(
        &self,
        operation: crate::store::LifecycleRecord,
    ) -> TestResult<Sandbox> {
        let profile = seed_sandbox_profile_with_capacity(
            self.profiles.as_ref(),
            SandboxProfileId::generate(),
            vec![],
            vec![],
            capacity(),
            operation.input.resolved_image,
        )
        .await;
        Ok(self
            .service
            .create_for_lifecycle(
                &owner(),
                CreateSandbox {
                    profile,
                    name: Some(operation.input.request.name),
                    confirmed_digest: None,
                    origin: SandboxOrigin::Lifecycle {
                        operation_id: operation.id,
                    },
                },
            )
            .await?)
    }

    pub(super) async fn request_update(
        &self,
        id: &SandboxId,
        version: &str,
    ) -> TestResult<Sandbox> {
        let queued = self.updates.request(&owner(), id, version).await?;
        assert_eq!(
            queued
                .runtime_update_request()
                .map(|request| request.target_version.as_str()),
            Some(version)
        );
        let outcome = self
            .store
            .schedule_image_rotation(SandboxImageRotationRequest {
                sandbox_id: id.clone(),
                requested_at: layer2_core::now_rfc3339(),
                expected_runtime_version: version.into(),
            })
            .await?;
        assert_eq!(outcome, SandboxMaintenanceOutcome::Scheduled);
        self.sandbox(id).await
    }

    pub(in super::super) async fn drive_active(&self, id: &SandboxId) -> TestResult<Sandbox> {
        for _ in 0..24 {
            let sandbox = self.sandbox(id).await?;
            if sandbox.status == SandboxStatus::Active && sandbox.lifecycle.operation.is_none() {
                return Ok(sandbox);
            }
            if !self.worker.run_one(id, SystemTime::now()).await? {
                return Err(format!(
                    "Sandbox {id} has no due work in state {:?} at {:?}: {:?}",
                    sandbox.status, sandbox.work.step, sandbox.failure,
                )
                .into());
            }
        }
        Err(format!("Sandbox {id} did not become active within 24 worker steps").into())
    }
}

pub(super) fn owner() -> Caller {
    Caller::new(
        SubjectId::new("owner"),
        Authorization::new([Capability::CreateSandbox, Capability::UseOwnSandboxes]),
    )
}
