//! Exercise the production scheduler with real retained-home adoption/status.
use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, ResponseFrame};
use layer3_adapter::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use tokio::sync::Semaphore;

#[path = "binding_parent_race_tests.rs"]
mod parent_races;

type StatusHook = Arc<dyn Fn(bool) -> futures_util::future::BoxFuture<'static, ()> + Send + Sync>;

struct SlowBackend {
    inner: NativeBackend,
    adoption: Duration,
    status: Duration,
    permits: Arc<Semaphore>,
    status_hook: Option<StatusHook>,
}

#[async_trait::async_trait]
impl HostedBackend for SlowBackend {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.inner.provision(request).await
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        self.inner.inventory().await
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        let _permit = self.permits.clone().acquire_owned().await.unwrap();
        tokio::time::sleep(self.adoption).await;
        self.inner.adopt(handle, request).await
    }
    async fn reclaim(&self, observation: &RuntimeObservation) -> layer3_adapter::Result<()> {
        self.inner.reclaim(observation).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let _permit = self.permits.clone().acquire_owned().await.unwrap();
        let status = matches!(request, RequestFrame::RuntimeStatus(_));
        if status {
            tokio::time::sleep(self.status).await;
            if let Some(hook) = &self.status_hook {
                hook(false).await;
            }
        }
        let response = self.inner.exchange(handle, request).await;
        if status && response.is_ok() {
            if let Some(hook) = &self.status_hook {
                hook(true).await;
            }
        }
        response
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.inner
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.inner.teardown(handle).await
    }
}

#[tokio::test(start_paused = true)]
async fn due_reserved_composite_adoption_finishes_in_six_to_nineteen_seconds_with_unchanged_boot() {
    for seconds in [6, 19, 21] {
        let (fixture, active, now) = active_fixture().await;
        let permits = Arc::new(Semaphore::new(1));
        let backend = SlowBackend {
            inner: fixture.backend.clone(),
            adoption: Duration::from_secs(seconds / 2),
            status: Duration::from_secs(seconds - seconds / 2),
            permits: permits.clone(),
            status_hook: None,
        };
        let mut config = CodexSandboxSpec::default();
        config.deployment.image.uri = runtime_image();
        let adapter = Arc::new(
            CodexAdapter::with_backend(backend, config).with_home_volumes(fixture.homes.clone()),
        );
        let reconciler = SandboxReconciler::new(
            fixture.harness.store.clone(),
            adapter,
            Arc::new(fixture.harness.policy.clone()),
            "test".into(),
        );
        let started = tokio::time::Instant::now();
        assert!(reconciler.run_due(now).await.unwrap());
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        assert_eq!(permits.available_permits(), 1);
        assert_eq!(fixture.harness.stored(&active.id).await, active);
        tokio::time::advance(Duration::from_secs(30)).await;
        // Inventory/authority discovery uses the first slice. No slow call has
        // started or lost its opportunity when it yields at the binding cursor.
        let started = tokio::time::Instant::now();
        assert!(!reconciler.run_due(now).await.unwrap());
        assert_eq!(started.elapsed(), Duration::ZERO);
        let started = tokio::time::Instant::now();
        assert!(!reconciler.run_due(now).await.unwrap());
        assert_eq!(started.elapsed(), Duration::from_secs(seconds.min(20)));
        assert_eq!(permits.available_permits(), 1);
        assert_eq!(fixture.harness.stored(&active.id).await, active);
        assert!(reconciler.run_due(now).await.unwrap());
        // Successful adoption resets the hint; another first attempt is still
        // bounded by five seconds. A >20s refusal remains held through backoff.
        let started = tokio::time::Instant::now();
        assert!(reconciler.run_due(now).await.unwrap());
        assert_eq!(
            started.elapsed(),
            Duration::from_secs(if seconds <= 20 { 5 } else { 0 })
        );
        assert_eq!(permits.available_permits(), 1);
        assert_eq!(fixture.harness.stored(&active.id).await, active);
        assert!(!fixture
            .backend
            .calls
            .snapshot()
            .contains(&"delete-retained".into()));
    }
}
