use super::*;
use layer3_adapter::hosted::{RuntimeBackends, RuntimeRoutingStore};
use layer3_adapter::{
    HomeVolumePolicy, HomeVolumeScope, RoutedHomeVolumes, RuntimeProvisionRequest, SandboxSpec,
};

struct LocalHome {
    home: HomeVolumeAttachment,
    removed: AtomicBool,
}
impl LocalHome {
    fn new() -> Arc<Self> {
        let request = HomeVolumeRequest {
            volume_id: "legacy-home".into(),
            sandbox_id: "gone-owner".into(),
            policy: home_tests::home_policy(),
            scope: None,
        };
        Arc::new(Self {
            home: HomeVolumeAttachment {
                receipt: HomeVolumeReceipt {
                    native_authority: None,
                    volume_id: request.volume_id.clone(),
                    request_sha256: request.computed_sha256(),
                    storage_reference: "legacy-pvc".into(),
                    storage_generation: "legacy-uid".into(),
                },
                request,
            },
            removed: AtomicBool::new(false),
        })
    }
}
#[async_trait::async_trait]
impl HomeVolumeStore for LocalHome {
    fn allocation_scope(&self) -> Option<HomeVolumeScope> {
        Some(
            HomeVolumeScope::from_authority(
                "kubernetes-pvc-v1",
                &["local", "runtime", "deployment"],
            )
            .unwrap(),
        )
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<HomeVolumeAttachment>> {
        Ok(vec![self.home.clone()])
    }
    async fn prepare(&self, _: &HomeVolumeRequest) -> layer3_adapter::Result<HomeVolumeReceipt> {
        panic!("no allocation")
    }
    async fn verify(&self, home: &HomeVolumeAttachment) -> layer3_adapter::Result<()> {
        assert_eq!(home, &self.home);
        Ok(())
    }
    async fn remove(
        &self,
        home: &HomeVolumeRequest,
        receipt: Option<&HomeVolumeReceipt>,
    ) -> layer3_adapter::Result<()> {
        assert_eq!(home, &self.home.request);
        assert_eq!(receipt, Some(&self.home.receipt));
        self.removed.store(true, Ordering::SeqCst);
        Ok(())
    }
}
struct NativeDefault;
#[async_trait::async_trait]
impl RuntimeRoutingStore for NativeDefault {
    async fn select(&self, _: &str, _: u64, _: bool) -> layer3_adapter::Result<bool> {
        Ok(true)
    }
}
fn adapter(harness: &Harness, local: Arc<LocalHome>, pools: Arc<Pools>) -> CodexAdapter {
    let homes = Arc::new(
        RoutedHomeVolumes::new(
            Some((local, home_tests::home_policy())),
            Some((
                pools.clone(),
                HomeVolumePolicy {
                    mode: Default::default(),
                    template_name: "remote-seed".into(),
                    template_generation: "remote-seed-uid".into(),
                    ..home_tests::home_policy()
                },
            )),
        )
        .unwrap(),
    );
    CodexAdapter::with_backend(
        RuntimeBackends {
            opensandbox: Some(Arc::new(harness.backend.clone())),
            agent_sandbox: Some(Arc::new(Backend {
                pools,
                backend: harness.backend.clone(),
            })),
            allocate_agent_sandbox: true,
            routing: Arc::new(NativeDefault),
            home_routes: Some(homes.clone()),
        },
        CodexSandboxSpec::default(),
    )
    .with_home_volumes(homes)
}

#[tokio::test]
async fn local_orphan_cleanup_never_queries_remote_native_pool() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let local = LocalHome::new();
    let pools = pools(&harness, &[]);
    let adapter = Arc::new(adapter(&harness, local.clone(), pools.clone()));
    crate::sandbox::reconciliation::SandboxReconciler::new(
        harness.store.clone(),
        adapter,
        Arc::new(harness.policy.clone()),
        "test".into(),
    )
    .initialize(SystemTime::now())
    .await
    .unwrap();
    assert!(local.removed.load(Ordering::SeqCst));
    assert!(pools.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_claim_recovery_and_cleanup_refuse_legacy_or_unknown_cluster_before_effects() {
    let harness = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let local = LocalHome::new();
    let pools = pools(&harness, &[]);
    let adapter = adapter(&harness, local.clone(), pools.clone());
    let runtime = RuntimeProvisionRequest {
        funded_until: None,
        native_canary_image: None,
        released: None,
        generation: 1,
        spec: SandboxSpec {
            sandbox_id: local.home.request.sandbox_id.clone(),
            created_by: Some("test".into()),
            ..Default::default()
        },
    };
    let new = SandboxHome::reserve(
        &SandboxId(runtime.spec.sandbox_id.clone()),
        home_tests::home_policy(),
    )
    .unwrap();
    let reserved = crate::sandbox::home_native::reserve(&adapter, &runtime, new)
        .await
        .unwrap();
    assert_eq!(reserved.request.scope, pools.allocation_scope());
    pools.calls.lock().unwrap().clear();
    for scope in [
        None,
        Some(HomeVolumeScope::from_authority("kubernetes-pvc-v1", &["unknown"]).unwrap()),
    ] {
        let mut old = reserved.clone();
        old.request.scope = scope;
        old.native_pool.as_mut().unwrap().request.home = old.request.clone();
        assert!(crate::sandbox::home_native::next(&adapter, &old)
            .await
            .is_err());
        assert!(crate::sandbox::home_native::next_cleanup(&adapter, &old)
            .await
            .is_err());
    }
    assert!(pools.calls.lock().unwrap().is_empty());
    assert!(pools.claim.lock().unwrap().is_none());
}
