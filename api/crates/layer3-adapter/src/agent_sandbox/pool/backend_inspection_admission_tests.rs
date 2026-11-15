//! Parent admission can hide reads, never an assignment or status dispatch.
use super::*;
use crate::agent_sandbox::pool::read_fixture::{Effects, Gate};
use crate::InspectionAdmission;
use std::{collections::BTreeMap, time::Duration};

struct ObservedTransport {
    inner: Arc<StatusTransport>,
    binds: AtomicUsize,
}
#[async_trait::async_trait]
impl HostedControlTransport for ObservedTransport {
    async fn bind_runtime(&self, handle: &RuntimeHandle, sandbox: &str) -> Result<()> {
        self.binds.fetch_add(1, Ordering::SeqCst);
        self.inner.bind_runtime(handle, sandbox).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> Result<ResponseFrame> {
        self.inner.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("inspection must not open an Agent")
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("inspection must not retire a runtime")
    }
}

struct Fixture {
    original: InspectionFixture,
    effects: Arc<Effects>,
    backend: Arc<AgentSandboxBackend>,
    transport: Arc<ObservedTransport>,
}
impl Fixture {
    async fn new() -> Self {
        Self::with_read_delay(Duration::ZERO).await
    }
    async fn with_read_delay(delay: Duration) -> Self {
        let original = inspection_fixture(true).await;
        let effects = Arc::new(Effects::with_delays(
            &original.bound.fixture,
            BTreeMap::new(),
            if delay.is_zero() {
                BTreeMap::new()
            } else {
                BTreeMap::from([(VOLUME.into(), delay)])
            },
        ));
        let client = AgentSandboxClient::new(
            effects.client.clone(),
            original.bound.fixture.client.config.clone(),
            "fixture",
        )
        .unwrap();
        let transport = Arc::new(ObservedTransport {
            inner: original.transport.clone(),
            binds: AtomicUsize::new(0),
        });
        let backend = Arc::new(AgentSandboxBackend::new(client, transport.clone()));
        backend
            .provision(original.bound.request.clone())
            .await
            .unwrap();
        transport.binds.store(0, Ordering::SeqCst);
        effects.calls.lock().unwrap().clear();
        Self {
            original,
            effects,
            backend,
            transport,
        }
    }
    fn start(
        &self,
        admission: InspectionAdmission,
    ) -> tokio::task::JoinHandle<Result<Option<RuntimeInspection>>> {
        let backend = self.backend.clone();
        let handle = self.original.bound.runtime.clone();
        let request = self.original.bound.request.clone();
        let home = self.original.home.clone();
        tokio::spawn(async move {
            backend
                .inspect_native_runtime_after(&handle, &request, &home, &admission)
                .await
        })
    }
    fn assert_no_authority(&self) {
        assert_eq!(self.transport.binds.load(Ordering::SeqCst), 0);
        assert_eq!(self.original.transport.calls.load(Ordering::SeqCst), 0);
        self.effects.assert_read_only();
    }
    async fn read_batch_complete(&self) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while self.effects.calls.lock().unwrap().len() < 6 {
                tokio::task::yield_now().await;
            }
            // The final immediate in-memory response gets polled before the
            // caller changes its resource. The production admission wait is
            // additionally observed by the task remaining pending below.
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn native_reads_overlap_parent_validation_without_early_binding_or_status() {
    let fixture = Fixture::new().await;
    let graph = Gate::new();
    fixture
        .effects
        .arm_read_gates(BTreeMap::from([(VOLUME.into(), graph.clone())]));
    let (release, admission) = InspectionAdmission::pending();
    let task = fixture.start(admission);
    graph.wait().await;
    fixture.assert_no_authority();
    release.allow();
    graph.respond();
    let result = task.await.unwrap().unwrap().unwrap();
    assert_eq!(result.observation.handle, fixture.original.bound.runtime);
    assert!(result.status.unwrap().credential_ready);
    assert_eq!(fixture.transport.binds.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.original.transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.effects.calls.lock().unwrap().len(), 12);
    fixture.effects.assert_read_only();
}

#[tokio::test]
async fn completed_observation_is_repeated_after_parent_admission() {
    let fixture = Fixture::new().await;
    let (release, admission) = InspectionAdmission::pending();
    let task = fixture.start(admission);
    fixture.read_batch_complete().await;
    assert!(!task.is_finished());
    fixture.assert_no_authority();
    release.allow();
    assert!(task.await.unwrap().unwrap().is_some());
    assert_eq!(fixture.effects.calls.lock().unwrap().len(), 18);
    assert_eq!(fixture.transport.binds.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.original.transport.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replacement_while_waiting_for_parent_cannot_authorize_status() {
    let fixture = Fixture::new().await;
    let (release, admission) = InspectionAdmission::pending();
    let task = fixture.start(admission);
    fixture.read_batch_complete().await;
    fixture.assert_no_authority();
    fixture
        .original
        .bound
        .fixture
        .state
        .lock()
        .unwrap()
        .rows
        .get_mut(VOLUME)
        .unwrap()["metadata"]["uid"] = json!("replaced-before-admission");
    release.allow();
    assert!(task.await.unwrap().is_err());
    fixture.assert_no_authority();
}

#[tokio::test]
async fn refusing_parent_drops_all_preparation_without_binding_or_dispatch() {
    let fixture = Fixture::new().await;
    let (release, admission) = InspectionAdmission::pending();
    let task = fixture.start(admission);
    fixture.read_batch_complete().await;
    drop(release);
    assert!(matches!(
        task.await.unwrap(),
        Err(AdapterError::Rejected(_))
    ));
    fixture.assert_no_authority();
}

#[tokio::test]
async fn cancelling_parent_preparation_leaves_next_inspection_usable() {
    let fixture = Fixture::new().await;
    let graph = Gate::new();
    fixture
        .effects
        .arm_read_gates(BTreeMap::from([(VOLUME.into(), graph.clone())]));
    let (_release, admission) = InspectionAdmission::pending();
    let task = fixture.start(admission);
    graph.wait().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    fixture.assert_no_authority();
    assert!(fixture
        .start(InspectionAdmission::ready())
        .await
        .unwrap()
        .unwrap()
        .is_some());
    assert_eq!(fixture.original.transport.calls.load(Ordering::SeqCst), 1);
}

#[path = "backend_inspection_profile_tests.rs"]
mod profile;

#[tokio::test]
async fn unsupported_native_optimization_waits_before_ordinary_adoption() {
    let fixture = inspection_fixture(true).await;
    // A non-native backend has no speculative path. The same outer adapter
    // gate must block its original full adoption, including an early refusal.
    let (adapter, _) = CodexAdapter::in_memory(CodexSandboxSpec::default());
    let (release, admission) = InspectionAdmission::pending();
    let mut request = fixture.adoption.clone();
    request.spec.home_volume = None;
    let pending = adapter.inspect_runtime_after(&request, &admission);
    tokio::pin!(pending);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), &mut pending)
            .await
            .is_err()
    );
    drop(release);
    assert!(matches!(pending.await, Err(AdapterError::Rejected(_))));
}
