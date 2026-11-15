use super::*;
use crate::{
    hosted::{HostedAgentOpenSettings, HostedBackend},
    provider::{ProviderImage, ProviderSandboxRequest},
    wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame},
    *,
};

struct RoutingBackend(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl HostedBackend for RoutingBackend {
    async fn reserve_home_request(
        &self,
        _: &ProviderSandboxRequest,
        home: &HomeVolumeRequest,
    ) -> Result<HomeVolumeRequest> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(home.clone())
    }
    async fn verify_home_route(
        &self,
        _: &ProviderSandboxRequest,
        _: &HomeVolumeRequest,
    ) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn validate_funding(
        &self,
        _: &ProviderSandboxRequest,
        _: Option<&RuntimeHandle>,
    ) -> Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        unreachable!()
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        unreachable!()
    }
    async fn teardown(&self, _: &RuntimeHandle) -> Result<()> {
        unreachable!()
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!()
    }
}

#[tokio::test]
async fn outer_routing_reservations_and_nominal_verification_are_admitted_before_side_effects() {
    let journal = Arc::new(Journal::default());
    journal.refuse.store(true, Ordering::SeqCst);
    let effects = Arc::new(AtomicUsize::new(0));
    let guarded = GuardedBackend::new(
        Arc::new(RoutingBackend(effects.clone())),
        authority(&journal),
    );
    let request = ProviderSandboxRequest {
        funded_until: None,
        image: ProviderImage {
            uri: "local/qualified".into(),
        },
        entrypoint: vec![],
        env: Default::default(),
        resource_limits: Default::default(),
        timeout: None,
        metadata: [
            ("cma/sandbox".into(), "original".into()),
            ("cma/runtime-generation".into(), "1".into()),
        ]
        .into(),
        volumes: vec![],
    };
    let home = HomeVolumeRequest {
        volume_id: "original-home".into(),
        sandbox_id: "original".into(),
        scope: None,
        policy: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "local".into(),
            capacity_gib: 1,
            template_name: "original-seed".into(),
            template_generation: "original-seed-uid".into(),
        },
    };
    assert!(guarded.reserve_home_request(&request, &home).await.is_err());
    assert!(guarded.verify_home_route(&request, &home).await.is_err());
    assert!(guarded.validate_funding(&request, None).await.is_err());
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    journal.refuse.store(false, Ordering::SeqCst);
    guarded.reserve_home_request(&request, &home).await.unwrap();
    guarded.verify_home_route(&request, &home).await.unwrap();
    guarded.validate_funding(&request, None).await.unwrap();
    assert_eq!(effects.load(Ordering::SeqCst), 3);
    assert_eq!(journal.admitted.load(Ordering::SeqCst), 3);
    assert_eq!(journal.outcomes.lock().unwrap().len(), 3);
}
