use super::*;
use layer2_core::sandbox::SandboxHome;
use layer3_adapter::agent_sandbox::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::{
    AdapterError, HomeVolumeAttachment, HomeVolumeReceipt, HomeVolumeRequest, HomeVolumeStore,
};

#[path = "native_prepared_tests.rs"]
mod native_prepared_tests;

pub(super) struct Pools {
    store: Arc<InMemorySandboxStore>,
    claim: Mutex<Option<NativePoolClaim>>,
    disk: Mutex<Option<HomeVolumeAttachment>>,
    failures: Mutex<VecDeque<&'static str>>,
    calls: Mutex<Vec<&'static str>>,
    released_selections: Option<Mutex<Vec<String>>>,
    delay_claim: AtomicBool,
    delay_claim_reply: AtomicBool,
    pending_claim: AtomicBool,
    stall_observe: AtomicBool,
    observe_error: Mutex<Option<AdapterError>>,
    adopt_error: Mutex<Option<AdapterError>>,
    observation: cadence::ObservationTrace,
    observed: Notify,
    entered: Notify,
    resume: Notify,
}

fn unavailable() -> AdapterError {
    AdapterError::Unavailable("fixture response lost".into())
}

impl Pools {
    async fn home(&self, request: &HomeVolumeRequest) -> SandboxHome {
        self.store
            .get_sandbox(&SandboxId(request.sandbox_id.clone()))
            .await
            .unwrap()
            .unwrap()
            .lifecycle
            .home
            .as_deref()
            .unwrap()
            .clone()
    }
    fn lose(&self, operation: &'static str) -> layer3_adapter::Result<()> {
        self.calls.lock().unwrap().push(operation);
        let mut failures = self.failures.lock().unwrap();
        if failures.front() == Some(&operation) {
            failures.pop_front();
            Err(unavailable())
        } else {
            Ok(())
        }
    }
    pub(super) fn binding(home: &HomeVolumeRequest) -> NativePoolBinding {
        NativePoolBinding {
            spec: NativePoolSpec {
                cold_canary: false,
                image: layer3_adapter::RuntimeImage {
                    uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
                    labels: BTreeMap::from([
                        (layer3_adapter::RUNTIME_CONTRACT_LABEL.into(), "3".into()),
                        ("dev.chrono.cma.runtime-bootstrap".into(), "1".into()),
                        ("dev.chrono.cma.home-layout".into(), "2".into()),
                    ]),
                },
                cpu_millis: 1000,
                memory_mib: 2048,
                home: home.policy.clone(),
            },
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "runtime".into(),
                image_pull_secrets: vec![],
                template_network_policy: None,
                fs_group_change_policy: None,
            },
            name: "pool".into(),
            uid: "pool-uid".into(),
            template_name: "template".into(),
            template_uid: "template-uid".into(),
            config_sha256: "a".repeat(64),
        }
    }
}

#[async_trait::async_trait]
impl NativePoolControl for Pools {
    async fn inventory_claims(&self) -> layer3_adapter::Result<Vec<NativePoolClaim>> {
        Ok(self.claim.lock().unwrap().iter().cloned().collect())
    }
    async fn reclaim_claim(&self, claim: &NativePoolClaim) -> layer3_adapter::Result<()> {
        let home = self.home(&claim.request.home).await;
        assert!(home.released);
        assert_eq!(self.claim.lock().unwrap().as_ref(), Some(claim));
        *self.claim.lock().unwrap() = None;
        self.lose("reclaim")
    }
    async fn cleanup_retained_home(&self, _: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        assert!(self.claim.lock().unwrap().is_none());
        self.lose("retained")
    }
    fn plan(&self, _: &NativePoolSpec) -> layer3_adapter::Result<NativePoolConfiguration> {
        Err(unavailable())
    }
    async fn configure_planned(
        &self,
        _: &NativePoolConfiguration,
        _: u32,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        Err(unavailable())
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<NativePoolStatus>> {
        Ok(vec![])
    }
    async fn resize(
        &self,
        _: &NativePoolBinding,
        _: u32,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        Err(unavailable())
    }
    async fn claim(
        &self,
        request: &NativePoolClaimRequest,
    ) -> layer3_adapter::Result<NativePoolClaim> {
        assert_eq!(
            self.home(&request.home)
                .await
                .native_pool
                .as_ref()
                .unwrap()
                .request,
            *request,
            "persist selection and claim request before Kubernetes creation"
        );
        if self.delay_claim.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        let claim = {
            let mut stored = self.claim.lock().unwrap();
            stored
                .get_or_insert_with(|| NativePoolClaim {
                    request: request.clone(),
                    name: "claim".into(),
                    uid: "claim-uid".into(),
                    resource_version: "1".into(),
                    sandbox_name: None,
                })
                .clone()
        };
        assert_eq!(claim.request, *request);
        self.lose("claim")?;
        if self.delay_claim_reply.swap(false, Ordering::SeqCst) {
            self.entered.notify_one();
            self.resume.notified().await;
        }
        Ok(claim)
    }
    async fn observe_claim(
        &self,
        original: &NativePoolClaim,
    ) -> layer3_adapter::Result<NativePoolClaim> {
        let _read = self.observation.start();
        // Capture server state before response latency; a later assignment
        // requires another observation rather than changing an in-flight reply.
        let pending = self.pending_claim.load(Ordering::SeqCst);
        self.observation.respond().await;
        if self.stall_observe.load(Ordering::SeqCst) {
            return std::future::pending().await;
        }
        assert_eq!(
            self.home(&original.request.home)
                .await
                .native_pool
                .as_ref()
                .unwrap()
                .claim
                .as_ref(),
            Some(original)
        );
        let mut claim = original.clone();
        if !pending {
            claim.sandbox_name = Some("warm-member".into());
        }
        *self.claim.lock().unwrap() = Some(claim.clone());
        self.lose("observe")?;
        self.observed.notify_one();
        if let Some(error) = self.observe_error.lock().unwrap().take() {
            return Err(error);
        }
        Ok(claim)
    }
    async fn adopt_home(
        &self,
        original: &NativePoolClaim,
    ) -> layer3_adapter::Result<HomeVolumeAttachment> {
        assert_eq!(
            self.home(&original.request.home)
                .await
                .native_pool
                .as_ref()
                .unwrap()
                .claim
                .as_ref(),
            Some(original),
            "persist assigned Sandbox before physical adoption"
        );
        assert!(original.sandbox_name.is_some());
        if let Some(error) = self.adopt_error.lock().unwrap().take() {
            return Err(error);
        }
        let disk = HomeVolumeAttachment {
            request: original.request.home.clone(),
            receipt: HomeVolumeReceipt {
                native_authority: original.request.home.policy.is_native_vct().then(|| {
                    Box::new(NativePoolHomeOrigin {
                        claim: original.clone(),
                        sandbox_uid: "member-uid".into(),
                        pvc_uid: "pvc-uid".into(),
                    })
                }),
                volume_id: original.request.home.volume_id.clone(),
                request_sha256: original.request.home.computed_sha256(),
                storage_reference: "home-warm-member".into(),
                storage_generation: "pvc-uid".into(),
            },
        };
        *self.disk.lock().unwrap() = Some(disk.clone());
        self.lose("adopt")?;
        Ok(disk)
    }
    async fn plan_cleanup(
        &self,
        original: &NativePoolCleanup,
    ) -> layer3_adapter::Result<NativePoolCleanup> {
        let mut next = original.clone();
        if let Some(claim) = self.claim.lock().unwrap().clone() {
            assert_eq!(claim.request, original.request);
            next.claim = Some(claim);
        }
        if next
            .claim
            .as_ref()
            .is_some_and(|claim| claim.sandbox_name.is_some())
        {
            next.members = vec![NativePoolMember {
                pvc_uid: None,
                name: "warm-member".into(),
                uid: "member-uid".into(),
            }];
        }
        Ok(next)
    }
    async fn cleanup(&self, original: &NativePoolCleanup) -> layer3_adapter::Result<()> {
        assert_eq!(
            self.home(&original.request.home)
                .await
                .native_pool
                .as_ref()
                .unwrap()
                .cleanup
                .as_ref(),
            Some(original),
            "persist recovered claim/member before deletion"
        );
        *self.claim.lock().unwrap() = None;
        self.lose("cleanup")
    }
}

#[async_trait::async_trait]
impl HomeVolumeStore for Pools {
    fn allocation_scope(&self) -> Option<layer3_adapter::HomeVolumeScope> {
        Some(
            layer3_adapter::HomeVolumeScope::from_authority(
                "kubernetes-pvc-v1",
                &["remote", "runtime", "deployment"],
            )
            .unwrap(),
        )
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(self.disk.lock().unwrap().iter().cloned().collect())
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        panic!("native reservation cannot allocate ordinary PVC")
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        assert_eq!(
            self.home(&home.request).await.attachment().as_ref(),
            Some(home),
            "persist home before mounting/credentials"
        );
        assert_eq!(self.disk.lock().unwrap().as_ref(), Some(home));
        Ok(())
    }
    async fn remove(
        &self,
        request: &HomeVolumeRequest,
        _: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        assert!(
            self.home(request)
                .await
                .native_pool
                .as_ref()
                .unwrap()
                .cleaned
        );
        assert!(self.claim.lock().unwrap().is_none());
        *self.disk.lock().unwrap() = None;
        self.lose("remove")
    }
}

#[derive(Clone)]
struct Backend {
    pools: Arc<Pools>,
    backend: InMemoryHostedBackend,
}

#[async_trait::async_trait]
impl HostedBackend for Backend {
    async fn reserve_home_request(
        &self,
        _: &layer3_adapter::provider::ProviderSandboxRequest,
        home: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<HomeVolumeRequest> {
        let mut home = home.clone();
        home.scope = Some(layer3_adapter::HomeVolumeScope::from_authority(
            "kubernetes-pvc-v1",
            &["remote", "runtime", "deployment"],
        )?);
        if !home.policy.is_native_vct() {
            home.policy.template_name = "remote-seed".into();
            home.policy.template_generation = "remote-seed-uid".into();
        }
        Ok(home)
    }
    fn native_pools(&self) -> Option<&dyn NativePoolControl> {
        Some(self.pools.as_ref())
    }
    async fn select_native_pool(
        &self,
        request: &layer3_adapter::provider::ProviderSandboxRequest,
        image: &layer3_adapter::RuntimeImage,
        home: &HomeVolumeRequest,
    ) -> layer3_adapter::Result<Option<NativePoolBinding>> {
        self.pools.lose("select")?;
        let mut binding = Pools::binding(home);
        if let Some(selections) = &self.pools.released_selections {
            assert_eq!(request.image.uri, image.uri);
            selections.lock().unwrap().push(image.uri.clone());
            binding.spec.image = image.clone();
        }
        Ok(Some(binding))
    }
    async fn provision(
        &self,
        request: layer3_adapter::provider::ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        let home = self.pools.disk.lock().unwrap().clone().unwrap();
        let reserved = self.pools.home(&home.request).await;
        assert_eq!(reserved.attachment().as_ref(), Some(&home));
        assert_eq!(
            request.image.uri,
            reserved.native_pool.unwrap().request.pool.spec.image.uri
        );
        self.backend.provision(request).await
    }
    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<layer3_adapter::wire::ResponseFrame> {
        self.backend.exchange(handle, request).await
    }
    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: layer3_adapter::wire::FencedRequest<layer3_adapter::AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: tokio::sync::mpsc::Sender<layer3_adapter::ConversationUpdate>,
    ) -> layer3_adapter::Result<(
        layer3_adapter::ConversationChannel,
        layer3_adapter::wire::FencedReceipt<layer3_adapter::wire::AgentOpenReceipt>,
    )> {
        self.backend
            .open_agent(handle, launch, resume, settings, updates)
            .await
    }
    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<layer3_adapter::RuntimeObservation>> {
        self.backend.inventory().await
    }
    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &layer3_adapter::provider::ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<layer3_adapter::RuntimeObservation>> {
        self.backend.adopt(handle, request).await
    }
}

fn worker(harness: &Harness, pools: Arc<Pools>) -> Arc<SandboxWorker> {
    let config = SandboxWorkerConfig::new(
        "native-test",
        "test",
        runtime_image(),
        harness.policy.clone(),
    );
    worker_with_config(harness, pools, config)
}

fn worker_with_config(
    harness: &Harness,
    pools: Arc<Pools>,
    config: SandboxWorkerConfig,
) -> Arc<SandboxWorker> {
    let adapter = Arc::new(
        CodexAdapter::with_backend(
            Backend {
                pools: pools.clone(),
                backend: harness.backend.clone(),
            },
            CodexSandboxSpec::default(),
        )
        .with_home_volumes(pools),
    );
    SandboxWorker::new(
        SandboxWorkerDependencies {
            store: harness.store.clone(),
            adapter,
            credentials: harness.credentials.clone(),
            workspaces: Arc::new(WorkspaceRecorder {
                actions: harness.actions.clone(),
            }),
            agents: Arc::new(AgentStopRecorder {
                actions: harness.actions.clone(),
            }),
            registry: harness.registry.clone(),
            updates: DurableAgentUpdateRecorder::new(harness.store.clone()),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        config.with_home_policy(Some(home_tests::home_policy())),
    )
}

pub(super) fn pools(harness: &Harness, failures: &[&'static str]) -> Arc<Pools> {
    Arc::new(Pools {
        store: harness.store.clone(),
        claim: Mutex::new(None),
        disk: Mutex::new(None),
        failures: Mutex::new(failures.iter().copied().collect()),
        calls: Mutex::new(vec![]),
        released_selections: None,
        delay_claim: AtomicBool::new(false),
        delay_claim_reply: AtomicBool::new(false),
        pending_claim: AtomicBool::new(false),
        stall_observe: AtomicBool::new(false),
        observe_error: Mutex::new(None),
        adopt_error: Mutex::new(None),
        observation: Default::default(),
        observed: Notify::new(),
        entered: Notify::new(),
        resume: Notify::new(),
    })
}

#[path = "native_pool_lifecycle_tests.rs"]
mod lifecycle;

#[path = "native_home_routing_tests.rs"]
mod routing;

#[path = "native_home_admission_conflict_tests.rs"]
mod admission_conflict;

#[path = "native_claim_assignment_tests.rs"]
mod assignment;

#[path = "native_claim_cadence_tests.rs"]
mod cadence;

#[path = "native_pool_release_admission_tests.rs"]
mod release_admission;
