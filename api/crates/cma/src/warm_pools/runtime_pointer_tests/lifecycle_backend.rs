use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend, InMemoryHostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{
    AgentLaunch, ConversationChannel, ConversationUpdate, HomeVolumeAttachment, HomeVolumeReceipt,
    HomeVolumeRequest, HomeVolumeScope, HomeVolumeStore, RoutedHomeVolumes, RuntimeHandle,
    RuntimeImage, RuntimeObservation,
};

#[derive(Clone, Default)]
pub(super) struct HostedFixture {
    backend: InMemoryHostedBackend,
    runtimes: Arc<tokio::sync::Mutex<BTreeMap<RuntimeHandle, Runtime>>>,
}

struct Runtime {
    request: ProviderSandboxRequest,
    observation: RuntimeObservation,
    pod: Option<RuntimeHandle>,
}

impl HostedFixture {
    async fn start(
        &self,
        request: ProviderSandboxRequest,
        handle: RuntimeHandle,
    ) -> layer3_adapter::Result<Runtime> {
        let pod = self.backend.provision(request.clone()).await?;
        let mut observation = self
            .backend
            .inventory()
            .await?
            .into_iter()
            .find(|row| row.handle == pod)
            .unwrap();
        observation.handle = handle;
        Ok(Runtime {
            request,
            observation,
            pod: Some(pod),
        })
    }

    async fn pod(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<RuntimeHandle> {
        self.runtimes
            .lock()
            .await
            .get(handle)
            .and_then(|runtime| runtime.pod.clone())
            .ok_or_else(|| {
                layer3_adapter::AdapterError::Unavailable("fixture Pod is suspended".into())
            })
    }
}

pub(super) struct Backend {
    pub(super) provider: Provider,
    pub(super) hosted: HostedFixture,
    pub(super) homes: Arc<RoutedHomeVolumes>,
}

#[async_trait]
impl HostedBackend for Backend {
    fn native_pools(&self) -> Option<&dyn NativePoolControl> {
        Some(&self.provider)
    }

    async fn reserve_home_request(
        &self,
        _: &ProviderSandboxRequest,
        home: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeRequest> {
        self.homes.reserve_for(true, home)
    }

    async fn select_native_pool(
        &self,
        request: &ProviderSandboxRequest,
        image: &RuntimeImage,
        home: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<Option<NativePoolBinding>> {
        assert_eq!(request.image.uri, image.uri);
        let inventory = NativePoolControl::inventory(&self.provider).await?;
        let pool = inventory.into_iter().find(|pool| {
            let spec = &pool.binding.spec;
            &spec.image == image
                && spec.home == home.policy
                && request.resource_limits.get("cpu") == Some(&format!("{}m", spec.cpu_millis))
                && request.resource_limits.get("memory") == Some(&format!("{}Mi", spec.memory_mib))
                && pool.desired_replicas > 0
                && pool.ready_replicas > 0
                && pool.observed
        });
        pool.map(|pool| Some(pool.binding)).ok_or_else(|| {
            layer3_adapter::AdapterError::Unavailable("fixture pool not ready".into())
        })
    }

    async fn provision(&self, _: ProviderSandboxRequest) -> layer3_adapter::Result<RuntimeHandle> {
        unreachable!("the fixture requires a persisted home attachment")
    }

    async fn provision_with_home(
        &self,
        request: ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.provider.verify(home).await?;
        let origin = home.receipt.native_authority.as_ref().unwrap();
        assert_eq!(request.image.uri, origin.claim.request.pool.spec.image.uri);
        let handle = RuntimeHandle::new(format!(
            "agent-sandbox:{}:{}",
            origin.claim.sandbox_name.as_ref().unwrap(),
            origin.sandbox_uid
        ));
        let mut runtimes = self.hosted.runtimes.lock().await;
        if let Some(existing) = runtimes.get(&handle) {
            assert_eq!(existing.request, request);
            assert!(existing.pod.is_some());
        } else {
            let runtime = self.hosted.start(request, handle.clone()).await?;
            runtimes.insert(handle.clone(), runtime);
        }
        Ok(handle)
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        self.hosted
            .backend
            .exchange(&self.hosted.pod(handle).await?, request)
            .await
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        self.hosted
            .backend
            .open_agent(
                &self.hosted.pod(handle).await?,
                launch,
                resume,
                settings,
                updates,
            )
            .await
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        let mut runtimes = self.hosted.runtimes.lock().await;
        if let Some(runtime) = runtimes.get(handle) {
            if let Some(pod) = &runtime.pod {
                self.hosted.backend.teardown(pod).await?;
            }
            runtimes.remove(handle);
        }
        Ok(())
    }

    async fn suspend(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<()> {
        let mut runtimes = self.hosted.runtimes.lock().await;
        let runtime = runtimes.get_mut(handle).expect("known native Sandbox");
        assert_eq!(&runtime.request, request);
        if let Some(pod) = runtime.pod.as_ref() {
            self.hosted.backend.teardown(pod).await?;
            runtime.pod = None;
            runtime.observation.lifecycle.state = "suspended".into();
        }
        Ok(())
    }

    async fn verify_suspended(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<()> {
        let runtimes = self.hosted.runtimes.lock().await;
        let runtime = runtimes.get(handle).expect("known native Sandbox");
        assert_eq!(&runtime.request, request);
        assert!(runtime.pod.is_none());
        Ok(())
    }

    async fn upgrade_with_home(
        &self,
        handle: &RuntimeHandle,
        previous: &ProviderSandboxRequest,
        next: &ProviderSandboxRequest,
        home: &HomeVolumeAttachment,
    ) -> layer3_adapter::Result<()> {
        self.provider.verify(home).await?;
        assert_ne!(previous.image.uri, next.image.uri);
        let mut runtimes = self.hosted.runtimes.lock().await;
        let runtime = runtimes.get_mut(handle).expect("known native Sandbox");
        if &runtime.request == next {
            assert!(runtime.pod.is_some());
            return Ok(());
        }
        assert_eq!(&runtime.request, previous);
        assert!(runtime.pod.is_none());
        *runtime = self.hosted.start(next.clone(), handle.clone()).await?;
        Ok(())
    }

    async fn inventory(&self) -> layer3_adapter::Result<Vec<RuntimeObservation>> {
        Ok(self
            .hosted
            .runtimes
            .lock()
            .await
            .values()
            .map(|runtime| runtime.observation.clone())
            .collect())
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        Ok(self
            .hosted
            .runtimes
            .lock()
            .await
            .get(handle)
            .map(|runtime| {
                assert_eq!(&runtime.request, request);
                runtime.observation.clone()
            }))
    }
}

pub(super) struct SnapshotFixture;

#[async_trait]
impl HomeVolumeStore for SnapshotFixture {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(HomeVolumeScope::from_authority("snapshot", &["fixture", "local"]).unwrap())
    }

    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(Vec::new())
    }

    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        unreachable!("this fixture routes new homes to the native provider")
    }

    async fn verify(&self, _: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        unreachable!("this fixture only retains native homes")
    }

    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        unreachable!("the fixture retains its original home")
    }
}

#[async_trait]
impl HomeVolumeStore for Provider {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(
            HomeVolumeScope::from_authority("native-vct-full-home-v2", &["fixture", DEPLOYMENT])
                .unwrap(),
        )
    }

    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        let claims = self.inventory_claims().await?;
        let mut homes = Vec::with_capacity(claims.len());
        for claim in claims {
            homes.push(self.adopt_home(&claim).await?);
        }
        Ok(homes)
    }

    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        unreachable!("the fixture obtains its home through the native pool Claim")
    }

    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        let origin = home.receipt.native_authority.as_ref().unwrap();
        assert_eq!(self.adopt_home(&origin.claim).await?, *home);
        Ok(())
    }

    async fn remove(
        &self,
        _: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        unreachable!("the fixture retains its original home")
    }
}
