use super::*;
use crate::sandbox::worker::{
    LiveAgentStopper, ProfileWorkspaceProvisioner, SandboxDependencyError, SandboxWorker,
    SandboxWorkerConfig, SandboxWorkerDependencies,
};
use crate::sandbox::LiveAgentRegistry;
use layer2_core::profiles::store::InMemoryProfileStore;
use layer2_core::sandbox::{
    AgentOperationService, DurableAgentUpdateRecorder, FixedSandboxCredentialPrerequisite,
    SandboxPolicy, SandboxService,
};
use layer3_adapter::{
    codex::{CodexAdapter, CodexSandboxSpec},
    RuntimeHandle,
};

struct EmptyChildren;
struct PendingChildren;
#[async_trait::async_trait]
impl LiveAgentStopper for PendingChildren {
    async fn stop_live_agents(
        &self,
        _: &Sandbox,
        _: Option<&RuntimeHandle>,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        Err(SandboxDependencyError::AgentsStopping)
    }
}
#[async_trait::async_trait]
impl LiveAgentStopper for EmptyChildren {
    async fn stop_live_agents(
        &self,
        _: &Sandbox,
        _: Option<&RuntimeHandle>,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        Ok(())
    }
}
#[async_trait::async_trait]
impl ProfileWorkspaceProvisioner for EmptyChildren {
    async fn create_profile_workspace(
        &self,
        _: &Sandbox,
        _: &RuntimeHandle,
        _: &WorkspaceProfileRef,
        _: u16,
        _: &str,
    ) -> Result<(), SandboxDependencyError> {
        panic!("retirement must not prepare a workspace")
    }
}

pub(super) struct ScanObserver {
    inner: Arc<ManagedSandboxCredentials>,
    pub scans: std::sync::atomic::AtomicUsize,
    pub hold_at: std::sync::atomic::AtomicUsize,
    pub entered: tokio::sync::Notify,
    pub resume: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl SandboxCredentialController for ScanObserver {
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        panic!("retirement must not mint")
    }
    async fn material(
        &self,
        id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.inner.material(id, generation).await
    }
    async fn custodied(&self, id: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        let rows = self.inner.custodied(id).await?;
        assert!(rows.is_empty());
        let count = self.scans.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        if self.hold_at.load(std::sync::atomic::Ordering::SeqCst) == count {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        Ok(rows)
    }
    async fn forget(&self, id: &SandboxId, generation: u64) -> Result<(), CredentialError> {
        self.inner.forget(id, generation).await
    }
    async fn revoke(&self, sandbox: &Sandbox, id: &str) -> Result<(), CredentialError> {
        self.inner.revoke(sandbox, id).await
    }
    async fn settle_revocations(&self, sandbox: &Sandbox) -> Result<(), CredentialError> {
        self.inner.settle_revocations(sandbox).await
    }
}

pub(super) async fn begin(f: &Fixture) -> (Arc<SandboxWorker>, Arc<ScanObserver>) {
    begin_with_store(f, Arc::new(peer(&f.name).await)).await
}

pub(super) fn native_custody_worker(
    f: &Fixture,
    enabled: bool,
) -> (Arc<SandboxWorker>, Arc<dyn SandboxCredentialController>) {
    let store = Arc::new(f.store.clone());
    let registry = Arc::new(LiveAgentRegistry::default());
    let spec = CodexSandboxSpec::default();
    let image = spec.deployment.image.uri.clone();
    let (adapter, _) = CodexAdapter::in_memory(spec);
    let adapter: Arc<dyn layer3_adapter::SandboxAdapter> = Arc::new(adapter);
    let credentials = crate::sandbox::legacy_credentials::LegacySandboxCredentials::new(
        f.controller.clone(),
        store.clone(),
    );
    let mut config = SandboxWorkerConfig::new(
        "retirement-custody",
        "work-contract",
        image,
        SandboxPolicy::default(),
    )
    .with_replicas(true);
    config.retry_delay = Duration::ZERO;
    config.retirement_finalizer = Some(Arc::new(crate::sandbox::NativeRetirementFinalizer::new(
        store.clone(),
        adapter.clone(),
        "work-contract".into(),
        store.clone(),
        enabled,
    )));
    let worker = SandboxWorker::new(
        SandboxWorkerDependencies {
            store: store.clone(),
            adapter,
            credentials: credentials.clone(),
            workspaces: Arc::new(EmptyChildren),
            agents: Arc::new(PendingChildren),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store, registry),
        },
        config,
    );
    (worker, credentials)
}

pub(super) async fn begin_with_store(
    f: &Fixture,
    store: Arc<MongoSandboxStore>,
) -> (Arc<SandboxWorker>, Arc<ScanObserver>) {
    begin_with_policy(
        f,
        store,
        SandboxPolicy {
            retention_grace: Duration::ZERO,
            ..SandboxPolicy::default()
        },
    )
    .await
}

pub(super) async fn begin_with_policy(
    f: &Fixture,
    store: Arc<MongoSandboxStore>,
    policy: SandboxPolicy,
) -> (Arc<SandboxWorker>, Arc<ScanObserver>) {
    let registry = Arc::new(LiveAgentRegistry::default());
    let spec = CodexSandboxSpec::default();
    let image = spec.deployment.image.uri.clone();
    let (adapter, _) = CodexAdapter::in_memory(spec);
    let credentials = Arc::new(ScanObserver {
        inner: f.controller.clone(),
        scans: Default::default(),
        hold_at: Default::default(),
        entered: Default::default(),
        resume: Default::default(),
    });
    let worker = SandboxWorker::new(
        SandboxWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(adapter),
            credentials: credentials.clone(),
            workspaces: Arc::new(EmptyChildren),
            agents: Arc::new(EmptyChildren),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store.clone(), registry),
        },
        SandboxWorkerConfig::new("retirement", "work-contract", image, policy.clone()),
    );
    let service = SandboxService::new(
        store.clone(),
        Arc::new(InMemoryProfileStore::default()),
        policy,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
    );
    let owner = Caller::new(
        f.claim.owner.clone(),
        Authorization::new([Capability::DeleteOwnSandbox]),
    );
    let retiring = service.delete(&owner, &f.claim.id).await.unwrap();
    assert_eq!(retiring.status, SandboxStatus::Retiring);
    (worker, credentials)
}

pub(super) async fn retire(f: &Fixture) -> (Arc<SandboxWorker>, Arc<ScanObserver>) {
    let (worker, credentials) = begin(f).await;
    for _ in 0..12 {
        let row = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
        if row.status == SandboxStatus::Retired {
            assert_eq!(row.work.step, SandboxWorkStep::WaitForPurge);
            return (worker, credentials);
        }
        assert!(worker
            .run_one(&f.claim.id, SystemTime::now())
            .await
            .unwrap());
    }
    panic!("retirement did not settle")
}
