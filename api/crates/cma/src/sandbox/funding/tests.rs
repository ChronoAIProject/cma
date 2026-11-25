use super::*;
use layer2_core::sandbox_usage::UsageEventKind;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};
mod evidence;
mod fixture;
mod http;
mod missing_http_tests;
mod missing_tests;
pub(crate) mod pilot;
mod pilot_http;
mod runtime_controls;
use fixture::sandbox;

fn at(n: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(n)
}
fn transient() -> FundingError {
    FundingError::Retryable("lost response".into())
}

#[derive(Default)]
struct MemoryState {
    values: Mutex<HashMap<String, SandboxBillingState>>,
    events: Mutex<Vec<UsageEvent>>,
    sandbox: Mutex<Option<Sandbox>>,
    writes: Mutex<Vec<SandboxBillingState>>,
    fail_save: AtomicBool,
    load_barrier: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    fail_adoption: AtomicBool,
    lose_adoption_ack: AtomicBool,
}
#[async_trait]
impl FundingStateStore for MemoryState {
    async fn load(&self, id: &SandboxId) -> Result<Option<SandboxBillingState>, FundingError> {
        let state = self.values.lock().unwrap().get(id.as_str()).cloned();
        let barrier = self.load_barrier.lock().unwrap().take();
        if let Some(barrier) = barrier {
            barrier.wait().await;
            barrier.wait().await;
        }
        Ok(state)
    }
    async fn save(&self, state: &mut SandboxBillingState) -> Result<(), FundingError> {
        if self.fail_save.swap(false, Ordering::SeqCst) {
            return Err(transient());
        }
        let mut values = self.values.lock().unwrap();
        let adoption = state.pending.is_none()
            && values
                .get(&state.sandbox_id)
                .is_some_and(|saved| saved.pending.is_some());
        if adoption && self.fail_adoption.swap(false, Ordering::SeqCst) {
            return Err(transient());
        }
        if values.get(&state.sandbox_id).map_or(0, |v| v.revision) != state.revision {
            return Err(FundingError::Conflict);
        }
        state.revision += 1;
        values.insert(state.sandbox_id.clone(), state.clone());
        self.writes.lock().unwrap().push(state.clone());
        if adoption && self.lose_adoption_ack.swap(false, Ordering::SeqCst) {
            return Err(transient());
        }
        Ok(())
    }
    async fn events(&self, _: &SandboxId, after: u64) -> Result<Vec<UsageEvent>, FundingError> {
        Ok(self
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.revision > after)
            .cloned()
            .collect())
    }
    async fn checkpoint(&self, _: &SandboxId, _: SystemTime) -> Result<(), FundingError> {
        Ok(())
    }
    async fn due(&self, now: u64) -> Result<Vec<SandboxBillingState>, FundingError> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.unbilled_generation.is_none() && s.retry_at <= now)
            .cloned()
            .collect())
    }
    async fn sandbox(&self, _: &SandboxId) -> Result<Option<Sandbox>, FundingError> {
        Ok(self.sandbox.lock().unwrap().clone())
    }
}
#[derive(Default)]
struct Control {
    deadlines: Mutex<Vec<u64>>,
    pauses: Mutex<Vec<String>>,
    fail_deadline: AtomicBool,
}
#[async_trait]
impl FundingControl for Control {
    async fn deadline(&self, runtime: FundedRuntime<'_>) -> Result<(), FundingError> {
        if self.fail_deadline.load(Ordering::SeqCst) {
            return Err(transient());
        }
        self.deadlines.lock().unwrap().push(runtime.until());
        Ok(())
    }
    async fn pause(
        &self,
        _: FundingRuntime<'_>,
        detail: &str,
        _: SystemTime,
    ) -> Result<(), FundingError> {
        self.pauses.lock().unwrap().push(detail.into());
        Ok(())
    }
}
struct Provider {
    store: Arc<MemoryState>,
    now: AtomicU64,
    quotes: AtomicU64,
    requests: Mutex<Vec<ReserveRequest>>,
    receipts: Mutex<HashMap<String, FundingWindow>>,
    recoveries: Mutex<Vec<String>>,
    evidence: Mutex<Vec<EvidenceRequest>>,
    fail_quote: Mutex<Option<FundingError>>,
    fail_reserve: Mutex<Option<FundingError>>,
    lose_reserve: AtomicBool,
    lose_evidence: AtomicBool,
    fail_recover: AtomicBool,
}
impl Provider {
    fn new(store: Arc<MemoryState>) -> Arc<Self> {
        Arc::new(Self {
            store,
            now: AtomicU64::new(100),
            quotes: AtomicU64::new(0),
            requests: Mutex::new(vec![]),
            receipts: Mutex::new(HashMap::new()),
            recoveries: Mutex::new(Vec::new()),
            evidence: Mutex::new(vec![]),
            fail_quote: Mutex::new(None),
            fail_reserve: Mutex::new(None),
            lose_reserve: AtomicBool::new(false),
            lose_evidence: AtomicBool::new(false),
            fail_recover: AtomicBool::new(false),
        })
    }
}
#[async_trait]
impl FundingProvider for Provider {
    async fn quote(&self, request: QuoteRequest) -> Result<FundingQuote, FundingError> {
        assert!(
            self.store
                .values
                .lock()
                .unwrap()
                .contains_key(&request.sandbox_id),
            "owner operation must be durable before quote"
        );
        self.quotes.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = self.fail_quote.lock().unwrap().clone() {
            return Err(error);
        }
        Ok(FundingQuote {
            quote_id: uuid::Uuid::new_v4().to_string(),
            owner_id: request.owner_id,
            sandbox_id: request.sandbox_id,
            maximum_allocation: request.allocation,
            accept_before: self.now.load(Ordering::SeqCst) + 900,
            receipt: serde_json::json!({"rate_card_version":1,"credits_per_unit":{"cpu":"0.000000010000"}}),
        })
    }
    async fn reserve(&self, request: ReserveRequest) -> Result<FundingWindow, FundingError> {
        assert_eq!(
            self.store.values.lock().unwrap()[&request.sandbox_id]
                .pending
                .as_ref(),
            Some(&request),
            "exact request must precede provider call"
        );
        self.requests.lock().unwrap().push(request.clone());
        if let Some(error) = self.fail_reserve.lock().unwrap().clone() {
            return Err(error);
        }
        let mut receipts = self.receipts.lock().unwrap();
        let previous = request
            .previous_window_id
            .as_ref()
            .and_then(|id| receipts.values().find(|w| &w.window_id == id));
        let from = previous.map_or(self.now.load(Ordering::SeqCst), |w| {
            w.covered_until.max(self.now.load(Ordering::SeqCst))
        });
        let receipt = receipts
            .entry(request.request_id.clone())
            .or_insert_with(|| FundingWindow {
                request_id: request.request_id.clone(),
                quote_id: request.quote_id.clone(),
                window_id: uuid::Uuid::new_v4().to_string(),
                owner_id: request.owner_id.clone(),
                sandbox_id: request.sandbox_id.clone(),
                runtime_generation: request.runtime_generation,
                allocation: request.allocation.clone(),
                covered_from: from,
                covered_until: from + request.seconds,
                receipt: serde_json::json!({"components":[]}),
            })
            .clone();
        if self.lose_reserve.swap(false, Ordering::SeqCst) {
            return Err(transient());
        }
        Ok(receipt)
    }
    async fn recover(&self, id: &str) -> Result<Option<FundingWindow>, FundingError> {
        self.recoveries.lock().unwrap().push(id.into());
        if self.fail_recover.load(Ordering::SeqCst) {
            return Err(transient());
        }
        Ok(self.receipts.lock().unwrap().get(id).cloned())
    }
    async fn settle(&self, request: EvidenceRequest) -> Result<(), FundingError> {
        let state = &self.store.values.lock().unwrap()[&request.sandbox_id];
        assert_eq!(
            state.windows[0].evidence.as_ref(),
            Some(&request),
            "freeze before first send"
        );
        self.evidence.lock().unwrap().push(request);
        if self.lose_evidence.swap(false, Ordering::SeqCst) {
            return Err(transient());
        }
        Ok(())
    }
}
struct Harness {
    state: Arc<MemoryState>,
    provider: Arc<Provider>,
    control: Arc<Control>,
    adapter: Arc<ProviderFundingAdapter>,
    sandbox: Sandbox,
}
impl Harness {
    fn new() -> Self {
        let state = Arc::new(MemoryState::default());
        let provider = Provider::new(state.clone());
        let control = Arc::new(Control::default());
        let adapter = ProviderFundingAdapter::new(provider.clone(), state.clone(), control.clone());
        let sandbox = sandbox();
        *state.sandbox.lock().unwrap() = Some(sandbox.clone());
        Self {
            state,
            provider,
            control,
            adapter,
            sandbox,
        }
    }
    async fn saved(&self) -> SandboxBillingState {
        self.state.load(&self.sandbox.id).await.unwrap().unwrap()
    }
    async fn fund(&self, now: u64) -> Result<Option<u64>, FundingError> {
        self.adapter.ensure_coverage(&self.sandbox, at(now)).await
    }
}

#[tokio::test]
async fn reserve_is_durable_idempotent_and_renews_only_at_boundary() {
    let h = Harness::new();
    assert_eq!(h.fund(100).await.unwrap(), Some(400));
    let first = h.saved().await;
    assert_eq!(
        uuid::Uuid::parse_str(&first.operation_id)
            .unwrap()
            .get_version_num(),
        4
    );
    assert_eq!(
        first.quote.as_ref().unwrap().maximum_allocation,
        maximum_allocation(&h.sandbox)
    );
    assert_eq!(h.fund(369).await.unwrap(), Some(400));
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
    assert_eq!(h.fund(370).await.unwrap(), Some(700));
    let requests = h.provider.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].operation_id, requests[1].operation_id);
    assert_eq!(requests[0].quote_id, requests[1].quote_id);
    assert_eq!(
        requests[1].previous_window_id,
        Some(first.current.unwrap().window_id)
    );
    assert_ne!(requests[0].request_id, requests[1].request_id);
    assert_eq!(*h.control.deadlines.lock().unwrap(), vec![400, 400, 700]);
}
#[tokio::test]
async fn lost_reservation_recovers_after_restart_without_another_charge() {
    let h = Harness::new();
    h.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(h.fund(100).await.is_err());
    let pending = h.saved().await.pending.unwrap();
    let restarted =
        ProviderFundingAdapter::new(h.provider.clone(), h.state.clone(), h.control.clone());
    assert_eq!(
        restarted
            .ensure_coverage(&h.sandbox, at(110))
            .await
            .unwrap(),
        Some(400)
    );
    assert!(h.saved().await.pending.is_none());
    assert_eq!(h.provider.requests.lock().unwrap().as_slice(), &[pending]);
    assert_eq!(h.control.pauses.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn absent_receipt_replays_exact_request_and_transport_failure_does_not() {
    let h = Harness::new();
    *h.provider.fail_reserve.lock().unwrap() = Some(transient());
    assert!(h.fund(100).await.is_err());
    let request = h.saved().await.pending.unwrap();
    h.provider.fail_recover.store(true, Ordering::SeqCst);
    assert!(h.fund(110).await.is_err());
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
    h.provider.fail_recover.store(false, Ordering::SeqCst);
    *h.provider.fail_reserve.lock().unwrap() = None;
    h.fund(130).await.unwrap();
    assert_eq!(
        h.provider.requests.lock().unwrap().as_slice(),
        &[request.clone(), request]
    );
}
#[tokio::test]
async fn no_receipt_no_deadline_and_retry_after_is_durable() {
    let h = Harness::new();
    *h.provider.fail_reserve.lock().unwrap() = Some(FundingError::RateLimited(90));
    assert!(h.fund(100).await.is_err());
    assert_eq!(h.saved().await.retry_at, 190);
    assert!(h.fund(189).await.is_err());
    assert_eq!(h.provider.requests.lock().unwrap().len(), 1);
    assert!(h.control.deadlines.lock().unwrap().is_empty());
}
#[tokio::test]
async fn expired_recovered_receipt_never_authorizes_runtime() {
    let h = Harness::new();
    h.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(h.fund(100).await.is_err());
    assert!(matches!(h.fund(401).await, Err(FundingError::Unfunded(_))));
    assert_eq!(h.saved().await.windows.len(), 1);
    assert!(h.control.deadlines.lock().unwrap().is_empty());
}
#[tokio::test]
async fn failed_durable_intent_makes_no_provider_calls() {
    let h = Harness::new();
    h.state.fail_save.store(true, Ordering::SeqCst);
    assert!(h.fund(100).await.is_err());
    assert_eq!(h.provider.quotes.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn deadline_failure_denies_runtime_and_sweep_requests_pause() {
    let h = Harness::new();
    h.control.fail_deadline.store(true, Ordering::SeqCst);
    assert!(h.fund(100).await.is_err());
    assert!(h.saved().await.current.is_some());
    assert_eq!(h.adapter.renew_due(at(101)).await.unwrap(), 0);
    assert_eq!(h.control.pauses.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn stop_cannot_restart_same_generation_or_discard_ambiguous_receipt() {
    let h = Harness::new();
    h.provider.lose_reserve.store(true, Ordering::SeqCst);
    assert!(h.fund(100).await.is_err());
    h.adapter.settle(&h.sandbox, at(101)).await.unwrap();
    assert!(h.fund(110).await.is_err());
    let mut retired = h.sandbox.clone();
    retired.runtime = None;
    retired.status = layer2_core::sandbox::SandboxStatus::Retired;
    retired.lifecycle.home.as_mut().unwrap().released = true;
    *h.state.sandbox.lock().unwrap() = Some(retired);
    assert_eq!(h.adapter.renew_due(at(110)).await.unwrap(), 1);
    let state = h.saved().await;
    assert!(state.pending.is_none());
    assert_eq!(state.windows.len(), 1);
    assert!(h.control.deadlines.lock().unwrap().is_empty());
}
#[tokio::test]
async fn generation_change_waits_for_coverage_and_evidence_then_requotes() {
    let mut h = Harness::new();
    h.fund(100).await.unwrap();
    h.adapter.settle(&h.sandbox, at(110)).await.unwrap();
    h.sandbox.lifecycle.runtime_generation_high_water = 2;
    assert!(matches!(h.fund(120).await, Err(FundingError::Retryable(_))));
    let mut state = h.saved().await;
    assert!(h
        .adapter
        .fund(&h.sandbox, &mut state, maximum_allocation(&h.sandbox), 401)
        .await
        .is_err());
    h.state
        .events
        .lock()
        .unwrap()
        .push(event(&h.sandbox, 1, 100, 400));
    h.adapter.emit_evidence(&mut state, 401).await.unwrap();
    h.provider.now.store(401, Ordering::SeqCst);
    h.adapter
        .fund(&h.sandbox, &mut state, maximum_allocation(&h.sandbox), 401)
        .await
        .unwrap();
    assert_eq!(state.runtime_generation, 2);
    assert!(!state.stopped);
    assert_eq!(h.provider.quotes.load(Ordering::SeqCst), 2);
    h.sandbox.lifecycle.runtime_generation_high_water = 4;
    assert!(matches!(
        h.adapter
            .fund(&h.sandbox, &mut state, maximum_allocation(&h.sandbox), 402)
            .await,
        Err(FundingError::Invalid(_))
    ));
}
#[tokio::test]
async fn stale_replica_cannot_overwrite_newer_funding_state() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    let mut stale = h.saved().await;
    h.adapter.settle(&h.sandbox, at(110)).await.unwrap();
    stale.last_error = Some("stale".into());
    assert_eq!(h.state.save(&mut stale).await, Err(FundingError::Conflict));
    assert!(h.saved().await.stopped);
}
#[tokio::test]
async fn disabled_adapter_makes_no_external_calls() {
    let h = Harness::new();
    let adapter = NoopFundingAdapter;
    assert_eq!(
        adapter.ensure_coverage(&h.sandbox, at(100)).await.unwrap(),
        None
    );
    adapter.settle(&h.sandbox, at(101)).await.unwrap();
    assert_eq!(adapter.renew_due(at(400)).await.unwrap(), 0);
    assert!(h.state.values.lock().unwrap().is_empty());
    assert!(h.provider.requests.lock().unwrap().is_empty());
}
fn event(sandbox: &Sandbox, revision: u64, from: u64, until: u64) -> UsageEvent {
    let allocation = maximum_allocation(sandbox);
    UsageEvent {
        sandbox_id: sandbox.id.clone(),
        owner: sandbox.owner.clone(),
        revision,
        source_seq: revision,
        kind: UsageEventKind::Checkpoint,
        started_at: from,
        ended_at: until,
        quantity: allocation.usage(until - from).unwrap(),
        next_allocation: allocation.clone(),
        allocation,
        status: "active".into(),
    }
}

mod mongo;

#[tokio::test(start_paused = true)]
async fn funding_sweep_shutdown_cancels_pending_wait_without_an_extra_tick() {
    let (stop, receiver) = tokio::sync::watch::channel(false);
    let worker = FundingRenewalWorker::new(Arc::new(NoopFundingAdapter)).spawn(receiver);
    tokio::task::yield_now().await;
    stop.send(true).unwrap();
    worker.await.unwrap();
}

#[tokio::test]
async fn disk_only_receipt_cannot_authorize_a_delayed_compute_start() {
    let h = Harness::new();
    h.fund(100).await.unwrap();
    let mut pending = h.sandbox.clone();
    pending.runtime = None;
    *h.state.sandbox.lock().unwrap() = Some(pending);
    h.adapter.renew_due(at(370)).await.unwrap();
    let state = h.saved().await;
    assert!(!state.current.unwrap().allocation.running);
    let deadlines = h.control.deadlines.lock().unwrap().len();
    assert!(matches!(h.fund(380).await, Err(FundingError::Unfunded(_))));
    assert_eq!(h.control.deadlines.lock().unwrap().len(), deadlines);
}
