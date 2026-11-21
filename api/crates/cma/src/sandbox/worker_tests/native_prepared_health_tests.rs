use super::*;
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{
    AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeInspection, RuntimeStatusRequest,
};

#[derive(Clone, Copy)]
enum Observation {
    Exact,
    ReplacedBoot,
    Credential,
    MissingProcess,
    ExtraProcess,
    RestorePending,
    ForeignHome,
    ForeignImage,
    MissingStatus,
}

struct Probe {
    observation: Observation,
    inspections: AtomicUsize,
    plain_adoptions: AtomicUsize,
    separate_status: AtomicUsize,
    reconnects: AtomicUsize,
}

struct CoherentBackend {
    backend: InMemoryHostedBackend,
    probe: Arc<Probe>,
}

#[async_trait::async_trait]
impl HostedBackend for CoherentBackend {
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<layer3_adapter::RuntimeObservation>> {
        self.probe.plain_adoptions.fetch_add(1, Ordering::SeqCst);
        self.backend.adopt(handle, request).await
    }

    async fn inspect_native_runtime(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> layer3_adapter::Result<Option<RuntimeInspection>> {
        self.probe.inspections.fetch_add(1, Ordering::SeqCst);
        let Some(mut observation) = self.backend.adopt(handle, request).await? else {
            return Ok(None);
        };
        let response = self
            .backend
            .exchange(
                handle,
                RequestFrame::RuntimeStatus(RuntimeStatusRequest {
                    sandbox_id: observation.sandbox_id.clone(),
                    generation: observation.generation,
                    home_identity: home.identity()?,
                }),
            )
            .await?;
        let ResponseFrame::RuntimeStatus(mut status) = response else {
            panic!("status fixture")
        };
        match self.probe.observation {
            Observation::Exact => {}
            Observation::ReplacedBoot => status.boot_id = "f".repeat(32),
            Observation::Credential => status.credential_ready = true,
            Observation::MissingProcess => status.agent_processes = 0,
            Observation::ExtraProcess => status.agent_processes = 2,
            Observation::RestorePending => {
                status.installation_ready = false;
                status.restore_pending = true;
                status.agent_processes = 0;
            }
            Observation::ForeignHome => status.home_identity = "x".repeat(43),
            Observation::ForeignImage => observation.image_sha256 = "x".repeat(43),
            Observation::MissingStatus => {
                return Ok(Some(RuntimeInspection {
                    observation,
                    status: None,
                }))
            }
        }
        Ok(Some(RuntimeInspection {
            observation,
            status: Some(status),
        }))
    }

    async fn provision(&self, _: ProviderSandboxRequest) -> layer3_adapter::Result<RuntimeHandle> {
        panic!("health inspection must not provision")
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if matches!(request, RequestFrame::RuntimeStatus(_)) {
            self.probe.separate_status.fetch_add(1, Ordering::SeqCst);
        }
        self.backend.exchange(handle, request).await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        assert!(resume.is_some(), "health must verify the original session");
        self.probe.reconnects.fetch_add(1, Ordering::SeqCst);
        self.backend
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }

    async fn teardown(&self, _: &RuntimeHandle) -> layer3_adapter::Result<()> {
        panic!("health inspection must only return retirement intent")
    }
}

fn coherent(
    harness: &Harness,
    pools: Arc<Pools>,
    observation: Observation,
) -> (Arc<dyn SandboxAdapter>, Arc<Probe>) {
    let probe = Arc::new(Probe {
        observation,
        inspections: AtomicUsize::new(0),
        plain_adoptions: AtomicUsize::new(0),
        separate_status: AtomicUsize::new(0),
        reconnects: AtomicUsize::new(0),
    });
    let adapter = CodexAdapter::with_backend(
        CoherentBackend {
            backend: harness.backend.clone(),
            probe: probe.clone(),
        },
        CodexSandboxSpec::default(),
    )
    .with_home_volumes(pools);
    (Arc::new(adapter), probe)
}

async fn check(
    adapter: &dyn SandboxAdapter,
    prepared: &WarmSandbox,
) -> layer3_adapter::Result<WarmSandbox> {
    prepare_step(adapter, "test", prepared, &policy()).await
}

fn policy() -> layer3_adapter::HomeVolumePolicy {
    layer3_adapter::HomeVolumePolicy {
        mode: layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2,
        storage_class: "storage".into(),
        capacity_gib: 64,
        template_name: String::new(),
        template_generation: String::new(),
    }
}

async fn advance(
    harness: &Harness,
    pools: &Arc<Pools>,
    record: &WarmSandbox,
) -> layer3_adapter::Result<WarmSandbox> {
    let next = check(adapter(harness, pools.clone()).as_ref(), record).await?;
    Ok(persist(harness, next).await)
}

async fn ready(harness: &Harness, pools: &Arc<Pools>, mut record: WarmSandbox) -> WarmSandbox {
    for _ in 0..20 {
        record = advance(harness, pools, &record).await.unwrap();
        if record.state == WarmSandboxState::Ready {
            assert!(record.home.as_ref().unwrap().request.policy.is_native_vct());
            return record;
        }
    }
    panic!("prepared native VCT hierarchy did not become Ready");
}

#[tokio::test]
async fn coherent_prepared_health_reuses_status_and_still_checks_exact_session() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let prepared = ready(&harness, &pools, reservation(&harness).await).await;
    let (adapter, probe) = coherent(&harness, pools, Observation::Exact);
    let before = harness.backend.requests().await.len();
    assert_eq!(check(adapter.as_ref(), &prepared).await.unwrap(), prepared);
    assert_eq!(probe.inspections.load(Ordering::SeqCst), 1);
    assert_eq!(probe.plain_adoptions.load(Ordering::SeqCst), 0);
    assert_eq!(probe.separate_status.load(Ordering::SeqCst), 0);
    assert_eq!(probe.reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness.backend.requests().await[before..]
            .iter()
            .filter(|(_, r)| matches!(r, RequestFrame::RuntimeStatus(_)))
            .count(),
        1
    );
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn coherent_prepared_health_retires_changed_process_credential_or_agent_count() {
    for observation in [
        Observation::ReplacedBoot,
        Observation::Credential,
        Observation::MissingProcess,
        Observation::ExtraProcess,
        Observation::RestorePending,
    ] {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let pools = pools(&harness, &[]);
        let prepared = ready(&harness, &pools, reservation(&harness).await).await;
        let (adapter, probe) = coherent(&harness, pools, observation);
        let next = check(adapter.as_ref(), &prepared).await.unwrap();
        let mut retired = prepared;
        retired.state = WarmSandboxState::Retiring;
        assert_eq!(next, retired);
        assert_eq!(probe.inspections.load(Ordering::SeqCst), 1);
        assert_eq!(probe.separate_status.load(Ordering::SeqCst), 0);
        assert_eq!(probe.reconnects.load(Ordering::SeqCst), 0);
        assert!(harness.actions.snapshot().is_empty());
    }
}

#[tokio::test]
async fn coherent_prepared_health_rejects_foreign_home_image_or_missing_status() {
    for observation in [
        Observation::ForeignHome,
        Observation::ForeignImage,
        Observation::MissingStatus,
    ] {
        let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let pools = pools(&harness, &[]);
        let prepared = ready(&harness, &pools, reservation(&harness).await).await;
        let (adapter, probe) = coherent(&harness, pools, observation);
        assert!(matches!(
            check(adapter.as_ref(), &prepared).await,
            Err(AdapterError::Rejected(_))
        ));
        assert_eq!(probe.reconnects.load(Ordering::SeqCst), 0);
        assert_eq!(probe.separate_status.load(Ordering::SeqCst), 0);
        assert!(harness.actions.snapshot().is_empty());
    }
}

#[tokio::test]
async fn coherent_prepared_health_lost_status_remains_retryable_without_publication() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let prepared = ready(&harness, &pools, reservation(&harness).await).await;
    let (adapter, probe) = coherent(&harness, pools, Observation::Exact);
    harness
        .backend
        .lose_next_response(WireOperation::RuntimeStatus);
    assert!(matches!(
        check(adapter.as_ref(), &prepared).await,
        Err(AdapterError::Unavailable(_))
    ));
    assert_eq!(probe.reconnects.load(Ordering::SeqCst), 0);
    assert_eq!(check(adapter.as_ref(), &prepared).await.unwrap(), prepared);
    assert_eq!(probe.inspections.load(Ordering::SeqCst), 2);
    assert_eq!(probe.reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(probe.separate_status.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn coherent_prepared_inspection_allows_initial_installation_without_owner_state() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let mut installing = reservation(&harness).await;
    while installing.state == WarmSandboxState::Preparing {
        installing = advance(&harness, &pools, &installing).await.unwrap();
    }
    assert!(installing.installed_request_sha256.is_none());
    let (adapter, probe) = coherent(&harness, pools, Observation::Exact);
    let next = check(adapter.as_ref(), &installing).await.unwrap();
    assert_eq!(next.state, WarmSandboxState::Installing);
    assert!(next.installed_request_sha256.is_some());
    assert_eq!(next.runtime, installing.runtime);
    assert_eq!(probe.inspections.load(Ordering::SeqCst), 1);
    assert_eq!(probe.plain_adoptions.load(Ordering::SeqCst), 0);
    assert_eq!(probe.separate_status.load(Ordering::SeqCst), 0);
    assert_eq!(probe.reconnects.load(Ordering::SeqCst), 0);
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn coherent_prepared_health_absent_runtime_keeps_retirement_semantics() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let prepared = ready(&harness, &pools, reservation(&harness).await).await;
    let handle = RuntimeHandle::new(&prepared.runtime.as_ref().unwrap().runtime_id);
    harness.backend.teardown(&handle).await.unwrap();
    let (adapter, probe) = coherent(&harness, pools, Observation::Exact);
    let next = check(adapter.as_ref(), &prepared).await.unwrap();
    let mut retired = prepared;
    retired.state = WarmSandboxState::Retiring;
    assert_eq!(next, retired);
    assert_eq!(probe.inspections.load(Ordering::SeqCst), 1);
    assert_eq!(
        probe.plain_adoptions.load(Ordering::SeqCst),
        1,
        "missing coherent observation retains ordinary adoption fallback"
    );
    assert_eq!(probe.separate_status.load(Ordering::SeqCst), 0);
    assert_eq!(probe.reconnects.load(Ordering::SeqCst), 0);
    assert!(harness.actions.snapshot().is_empty());
}

#[tokio::test]
async fn coherent_prepared_health_session_failure_is_not_hidden_by_process_status() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let pools = pools(&harness, &[]);
    let prepared = ready(&harness, &pools, reservation(&harness).await).await;
    let (adapter, probe) = coherent(&harness, pools, Observation::Exact);
    harness.backend.fail_next(
        WireOperation::OpenAgent,
        "session_unavailable",
        "session health fixture",
    );
    assert!(check(adapter.as_ref(), &prepared).await.is_err());
    assert_eq!(probe.inspections.load(Ordering::SeqCst), 1);
    assert_eq!(probe.reconnects.load(Ordering::SeqCst), 1);
    assert_eq!(probe.separate_status.load(Ordering::SeqCst), 0);
    assert!(harness.actions.snapshot().is_empty());
}
