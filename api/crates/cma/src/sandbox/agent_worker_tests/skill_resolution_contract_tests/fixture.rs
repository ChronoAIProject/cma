use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::credential::{
    CredentialError, CredentialMaterial, SandboxCredentialController,
};

pub(super) struct Custody {
    material: Mutex<Option<CredentialMaterial>>,
    pending: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    pub calls: AtomicUsize,
}

impl Custody {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            material: Mutex::new(None),
            pending: Mutex::new(None),
            calls: AtomicUsize::new(0),
        })
    }

    pub fn stall(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (entered, receiver) = tokio::sync::oneshot::channel();
        *self.pending.lock().unwrap() = Some(entered);
        receiver
    }

    pub fn bind(&self, sandbox: &Sandbox) {
        *self.material.lock().unwrap() = Some(CredentialMaterial {
            source: sandbox.credential.source.clone(),
            sandbox_id: sandbox.id.clone(),
            owner: sandbox.owner.clone(),
            authority: sandbox.credential.current.clone().unwrap(),
            value: SecretValue::new("synthetic-skill-authority".into()),
            github_route: None,
        });
    }
}

#[async_trait::async_trait]
impl SandboxCredentialController for Custody {
    async fn material(
        &self,
        sandbox_id: &SandboxId,
        generation: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let pending = self.pending.lock().unwrap().take();
        if let Some(entered) = pending {
            entered
                .send(())
                .expect("fixture awaits the credential read");
            return std::future::pending().await;
        }
        let material = self.material.lock().unwrap().clone().unwrap();
        assert_eq!(material.sandbox_id, *sandbox_id);
        assert_eq!(material.authority.generation, generation);
        Ok(Some(material))
    }

    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        panic!("resolution cannot issue credentials")
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        panic!("resolution cannot enumerate credentials")
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        panic!("resolution cannot delete credentials")
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        panic!("resolution cannot revoke credentials")
    }
}

pub(super) fn reference(id: &str, version: &str) -> SkillRef {
    SkillRef {
        source: SkillSource::Ornn,
        id: SkillId::new(id),
        version: Some(version.into()),
    }
}

pub(super) fn reply(request: crate::dependency_test_support::Exchange, version: &str) {
    assert_eq!(
        request.headers["authorization"],
        "Bearer synthetic-skill-authority"
    );
    assert_eq!(request.method, axum::http::Method::GET);
    assert!(request.body.is_empty());
    assert_eq!(
        request.uri.query(),
        Some(format!("version={version}").as_str())
    );
    let id = request.uri.path().rsplit('/').next().unwrap().to_owned();
    request.json(serde_json::json!({
        "data": {"guid":id, "name":id, "version":version, "skillHash":"a".repeat(64)},
        "error":null
    }));
}

pub(super) async fn ready(peer: &mut Peer) -> (Harness, Agent, Arc<Custody>) {
    let custody = Custody::new();
    let resolver = crate::sandbox::agent_skills::test_support::scoped_peer(peer, custody.clone());
    let mut harness = Harness::with_upgrade_support(resolver.clone()).await;
    // Keep the worker lease longer than the 30-second dependency budget under test.
    let dependencies = AgentWorkerDependencies {
        store: harness.store.clone(),
        adapter: harness.adapter.clone(),
        skills: resolver,
        registry: harness.registry.clone(),
        updates: harness.updates.clone(),
        operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
    };
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.lease_duration = Duration::from_secs(60);
    config.retry_delay = Duration::from_secs(1);
    config.open_timeout = Duration::from_secs(2);
    harness.worker = AgentWorker::new(dependencies, config);
    let sandbox = harness
        .store
        .get_sandbox(&harness.workspace.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    custody.bind(&sandbox);
    let admitted = harness
        .admit("original", vec![reference("research", "1.0")])
        .await;
    let (ready, ()) = tokio::join!(harness.drive_ready(&admitted.id), async {
        reply(peer.next().await, "1.0")
    },);
    (harness, ready, custody)
}

pub(super) fn operations(harness: &Harness) -> (Arc<AgentOperationService>, Caller) {
    let caller = Caller::new(
        harness.workspace.owner.clone(),
        Authorization::new([Capability::UseOwnSandboxes]),
    );
    let operations = AgentOperationService::new(harness.store.clone(), harness.registry.clone())
        .with_skill_upgrades_enabled(true);
    (operations, caller)
}
