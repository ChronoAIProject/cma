use super::*;
use layer2_core::{
    profiles::{AgentProvider, ModelAccess, SandboxProfileContent, SandboxProfileSnapshot},
    sandbox::{store::*, *},
    SandboxProfileId, SubjectId,
};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
pub(super) enum Failure {
    Deadline,
    Unavailable,
    Corrupt,
}

pub(super) struct Store {
    pub(super) inner: InMemorySandboxStore,
    pub(super) calls: Mutex<Vec<(String, String)>>,
    pub(super) failure: Mutex<Option<(&'static str, String, Failure)>>,
    pub(super) delay: Duration,
}

impl Store {
    pub(super) fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            inner: InMemorySandboxStore::default(),
            calls: Mutex::default(),
            failure: Mutex::default(),
            delay,
        })
    }
    pub(super) fn fail(&self, phase: &'static str, key: &str, failure: Failure) {
        *self.failure.lock().unwrap() = Some((phase, key.into(), failure));
    }
    pub(super) fn keys(&self, phase: &str) -> Vec<String> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p == phase)
            .map(|(_, key)| key.clone())
            .collect()
    }
    async fn observe(&self, phase: &str, key: &str) -> Result<(), SandboxStoreError> {
        self.calls.lock().unwrap().push((phase.into(), key.into()));
        let failure = {
            let mut failure = self.failure.lock().unwrap();
            if failure
                .as_ref()
                .is_some_and(|(p, k, _)| *p == phase && k == key)
            {
                failure.take()
            } else {
                None
            }
        };
        match failure.map(|(_, _, f)| f) {
            Some(Failure::Deadline) => std::future::pending().await,
            Some(Failure::Unavailable) => Err(SandboxStoreError::Unavailable("fixture".into())),
            Some(Failure::Corrupt) => Err(SandboxStoreError::Corrupt("fixture".into())),
            None => {
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                Ok(())
            }
        }
    }
}

#[async_trait::async_trait]
impl SandboxStore for Store {
    async fn create_sandbox(&self, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.inner.create_sandbox(sandbox).await
    }
    async fn get_sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, SandboxStoreError> {
        self.observe("get", id.as_str()).await?;
        self.inner.get_sandbox(id).await
    }
    async fn save_sandbox(&self, revision: u64, sandbox: Sandbox) -> Result<(), SandboxStoreError> {
        self.inner.save_sandbox(revision, sandbox).await
    }
    async fn list_sandboxes(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.inner.list_sandboxes(owner, page).await
    }
}

#[async_trait::async_trait]
impl SandboxReconciliationStore for Store {
    async fn sandbox_runtime_authority(
        &self,
        page: &PageRequest,
    ) -> Result<Page<Sandbox>, SandboxStoreError> {
        self.observe("authority", page.cursor.as_deref().unwrap_or("start"))
            .await?;
        self.inner
            .sandbox_runtime_authority(&PageRequest {
                limit: 2,
                cursor: page.cursor.clone(),
            })
            .await
    }
    async fn recover_runtime_loss(
        &self,
        request: SandboxRuntimeLossRequest,
    ) -> Result<SandboxRuntimeLossOutcome, SandboxStoreError> {
        self.inner.recover_runtime_loss(request).await
    }
    async fn unowned_runtime_authority(
        &self,
        id: &SandboxId,
        _generation: u64,
        _runtime: &str,
    ) -> Result<bool, SandboxStoreError> {
        self.observe("runtime_fence", id.as_str()).await?;
        Ok(false)
    }
}

#[async_trait::async_trait]
impl SandboxMaintenanceStore for Store {
    async fn sandbox_maintenance_candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxId>, SandboxStoreError> {
        self.observe("candidates", page.cursor.as_deref().unwrap_or("start"))
            .await?;
        let page = self
            .inner
            .sandbox_runtime_authority(&PageRequest {
                limit: 2,
                cursor: page.cursor.clone(),
            })
            .await?;
        Ok(Page {
            items: page.items.into_iter().map(|s| s.id).collect(),
            next: page.next,
        })
    }
    async fn schedule_inactivity_pause(
        &self,
        request: SandboxInactivityPauseRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        self.observe("pause", request.sandbox_id.as_str()).await?;
        Ok(SandboxMaintenanceOutcome::NotEligible)
    }
    async fn schedule_image_rotation(
        &self,
        request: SandboxImageRotationRequest,
    ) -> Result<SandboxMaintenanceOutcome, SandboxStoreError> {
        self.inner.schedule_image_rotation(request).await
    }
}

pub(super) fn sandbox(index: usize) -> Sandbox {
    let now = "2026-09-05T00:00:00Z".to_string();
    Sandbox {
        id: SandboxId(format!("sbx_{index:03}")),
        owner: SubjectId::new("owner"),
        name: format!("fixture-{index}"),
        origin: SandboxOrigin::Manual,
        profile: SandboxProfileSnapshot {
            id: SandboxProfileId("sbp_fixture".into()),
            revision: 1,
            digest: "a".repeat(64),
            content: SandboxProfileContent {
                runtime_image: None,
                cpu_millis: 1000,
                memory_mib: 1024,
                nyxid_services: vec![],
                agent_provider: AgentProvider::default(),
                model_access: ModelAccess::ChronoaiManaged,
                max_workspaces: 1,
                max_agents_per_workspace: 1,
                workspace_profiles: vec![],
                software_install_commands: vec![],
                environment_variables: vec![],
            },
        },
        status: SandboxStatus::Installing,
        failure: None,
        runtime: None,
        credential: SandboxCredentialState {
            source: Default::default(),
            service_grants: vec![],
            service_scope: None,
            reaches: vec![],
            key_generation: 0,
            delivered: false,
            current: None,
            pending: None,
            revoke_debt: vec![],
            sign_in: SandboxSignInState::NotRequired,
        },
        work: SandboxWorkState::new(SandboxWorkStep::MintInitialCredential, now.clone()),
        limits: SandboxLimits {
            max_workspaces: 1,
            max_agents_per_workspace: 1,
        },
        lifecycle: Default::default(),
        last_activity_at: now.clone(),
        created_at: now.clone(),
        updated_at: now,
        retired_at: None,
        purge_after: None,
        revision: 1,
    }
}

pub(super) async fn setup(count: usize, delay: Duration) -> (Arc<Store>, SandboxReconciler) {
    let store = Store::new(delay);
    for index in 0..count {
        let sandbox = sandbox(index);
        sandbox.validate().unwrap();
        store.create_sandbox(sandbox).await.unwrap();
    }
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let policy = SandboxPolicy {
        max_sandboxes_per_owner: 10,
        max_sandboxes_per_visitor: 1,
        inactivity_pause_after_seconds: 900,
        checkpoint_interval_seconds: layer2_core::DEFAULT_CHECKPOINT_INTERVAL_SECONDS,
        retention_grace: Duration::from_secs(60),
        baseline_services: vec![],
    };
    let reconciler = SandboxReconciler::new(
        store.clone(),
        Arc::new(adapter),
        Arc::new(policy),
        "test".into(),
    );
    (store, reconciler)
}

#[derive(Clone, Default)]
pub(super) struct Output(pub(super) Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
