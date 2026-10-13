use super::*;
use layer3_adapter::agent_sandbox::{
    NativePoolClaim, NativePoolClaimRequest, NativePoolCleanup, NativePoolControl,
    NativePoolPodPolicy,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

fn target() -> PoolTarget {
    let spec = NativePoolSpec {
        image: RuntimeImage {
            uri: format!("registry.example/runtime@sha256:{}", "a".repeat(64)),
            labels: BTreeMap::from([
                (
                    layer3_adapter::RUNTIME_CONTRACT_LABEL.into(),
                    layer3_adapter::RUNTIME_CONTRACT_VERSION.to_string(),
                ),
                (
                    layer3_adapter::runtime_bootstrap::RUNTIME_BOOTSTRAP_LABEL.into(),
                    "1".into(),
                ),
            ]),
        },
        cpu_millis: 1000,
        memory_mib: 1024,
        home: HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "premium-rwo".into(),
            capacity_gib: 20,
            template_name: "seed".into(),
            template_generation: "seed-uid".into(),
        },
    };
    PoolTarget {
        id: "wp_test".into(),
        document_schema_version: 1,
        revision: 1,
        actor: "operator".into(),
        requested: ConfigureWarmPoolBody {
            cpu_millis: 1000,
            memory_mib: 1024,
            target_replicas: 3,
        },
        plan: NativePoolConfiguration {
            spec,
            pod_policy: NativePoolPodPolicy {
                runtime_class: None,
                service_account: "original-service".into(),
                image_pull_secrets: vec![],
            },
            namespace: "sandboxes".into(),
            deployment: "test".into(),
            config_sha256: "a".repeat(64),
        },
        binding: None,
        target_replicas: 3,
        applied_revision: 0,
        error: None,
        lease: Some("lease".into()),
        lease_until: mongodb::bson::DateTime::from_millis(i64::MAX),
        reconcile_after: mongodb::bson::DateTime::from_millis(0),
    }
}

struct Memory {
    target: Mutex<PoolTarget>,
    bind_response_loss: AtomicBool,
    calls: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl PoolTargetStore for Memory {
    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError> {
        Ok(vec![self.target.lock().unwrap().clone()])
    }
    async fn find(&self, _: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        Ok(Some(self.target.lock().unwrap().clone()))
    }
    async fn insert(&self, _: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        unreachable!()
    }
    async fn resize(&self, _: &str, _: &ResizeWarmPoolBody) -> Result<PoolTarget, WarmPoolError> {
        unreachable!()
    }
    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError> {
        Ok(Some(self.target.lock().unwrap().clone()))
    }
    async fn bind(&self, _: &PoolTarget, binding: &NativePoolBinding) -> Result<(), WarmPoolError> {
        self.calls.lock().unwrap().push("persist-binding".into());
        self.target.lock().unwrap().binding = Some(binding.clone());
        if self.bind_response_loss.swap(false, Ordering::SeqCst) {
            Err(WarmPoolError::Unavailable)
        } else {
            Ok(())
        }
    }
    async fn finish(&self, _: &PoolTarget, success: bool) -> Result<(), WarmPoolError> {
        self.calls.lock().unwrap().push(format!("finish:{success}"));
        Ok(())
    }
}

struct Provider {
    calls: Arc<Mutex<Vec<String>>>,
    original: NativePoolConfiguration,
    refuse_replaced_uid: AtomicBool,
}
impl Provider {
    fn binding(&self) -> NativePoolBinding {
        NativePoolBinding {
            spec: self.original.spec.clone(),
            pod_policy: self.original.pod_policy.clone(),
            name: "pool".into(),
            uid: "original-pool-uid".into(),
            template_name: "template".into(),
            template_uid: "original-template-uid".into(),
            config_sha256: self.original.config_sha256.clone(),
        }
    }
}
#[async_trait]
impl NativePoolControl for Provider {
    fn plan(&self, _: &NativePoolSpec) -> layer3_adapter::Result<NativePoolConfiguration> {
        unreachable!()
    }
    async fn configure_planned(
        &self,
        plan: &NativePoolConfiguration,
        replicas: u32,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        assert_eq!(plan, &self.original);
        assert_eq!(replicas, 0);
        self.calls.lock().unwrap().push("configure:0".into());
        Ok(NativePoolStatus {
            binding: self.binding(),
            desired_replicas: 0,
            ready_replicas: 0,
            observed: false,
        })
    }
    async fn inventory(&self) -> layer3_adapter::Result<Vec<NativePoolStatus>> {
        unreachable!()
    }
    async fn resize(
        &self,
        binding: &NativePoolBinding,
        replicas: u32,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        assert_eq!(binding, &self.binding());
        self.calls
            .lock()
            .unwrap()
            .push(format!("resize:{replicas}"));
        if self.refuse_replaced_uid.load(Ordering::SeqCst) {
            return Err(layer3_adapter::AdapterError::Rejected(
                "replaced UID".into(),
            ));
        }
        Ok(NativePoolStatus {
            binding: binding.clone(),
            desired_replicas: replicas,
            ready_replicas: 0,
            observed: false,
        })
    }
    async fn claim(&self, _: &NativePoolClaimRequest) -> layer3_adapter::Result<NativePoolClaim> {
        unreachable!()
    }
    async fn observe_claim(&self, _: &NativePoolClaim) -> layer3_adapter::Result<NativePoolClaim> {
        unreachable!()
    }
    async fn adopt_home(
        &self,
        _: &NativePoolClaim,
    ) -> layer3_adapter::Result<layer3_adapter::HomeVolumeAttachment> {
        unreachable!()
    }
    async fn plan_cleanup(
        &self,
        _: &NativePoolCleanup,
    ) -> layer3_adapter::Result<NativePoolCleanup> {
        unreachable!()
    }
    async fn cleanup(&self, _: &NativePoolCleanup) -> layer3_adapter::Result<()> {
        unreachable!()
    }
}

#[tokio::test]
async fn binding_is_durable_before_capacity_and_lost_ack_replays_original_uid() {
    let calls = Arc::new(Mutex::new(vec![]));
    let original = target();
    let provider = Provider {
        calls: calls.clone(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
    };
    let store = Memory {
        target: Mutex::new(original),
        bind_response_loss: AtomicBool::new(true),
        calls: calls.clone(),
    };
    assert!(reconcile_one(&store, &provider).await.unwrap());
    assert_eq!(
        *calls.lock().unwrap(),
        ["configure:0", "persist-binding", "finish:false"]
    );
    calls.lock().unwrap().clear();
    assert!(reconcile_one(&store, &provider).await.unwrap());
    assert_eq!(*calls.lock().unwrap(), ["resize:3", "finish:true"]);
    calls.lock().unwrap().clear();
    provider.refuse_replaced_uid.store(true, Ordering::SeqCst);
    assert!(reconcile_one(&store, &provider).await.unwrap());
    assert_eq!(*calls.lock().unwrap(), ["resize:3", "finish:false"]);
    assert_eq!(
        store.target.lock().unwrap().binding.as_ref().unwrap().uid,
        "original-pool-uid"
    );
}

#[test]
fn configure_replay_keeps_original_image_policy_and_rejects_changed_input() {
    let original = target();
    let body = original.requested.clone();
    let replayed = replay(original.clone(), &body).unwrap();
    assert_eq!(replayed.plan, original.plan);
    assert_ne!(target_id("a", "b"), target_id("b", "a"));
    assert!(matches!(
        replay(
            original,
            &ConfigureWarmPoolBody {
                target_replicas: 4,
                ..body
            }
        ),
        Err(WarmPoolError::Conflict)
    ));
}

#[test]
fn readiness_requires_original_binding_and_observed_current_target() {
    let mut original = target();
    let provider = Provider {
        calls: Arc::default(),
        original: original.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
    };
    let mut live = NativePoolStatus {
        binding: provider.binding(),
        desired_replicas: 3,
        ready_replicas: 3,
        observed: true,
    };
    original.binding = Some(live.binding.clone());
    original.applied_revision = original.revision;
    assert_eq!(original.view(Some(&live), true).status, "ready");
    live.binding.uid = "replacement".into();
    assert_eq!(original.view(Some(&live), true).ready_replicas, None);
    live.binding = original.binding.clone().unwrap();
    live.observed = false;
    assert_eq!(original.view(Some(&live), true).status, "pending");
    live.observed = true;
    live.desired_replicas = 2;
    assert!(!original.view(Some(&live), true).observed);
    let missing = original.view(None, false);
    assert_eq!(missing.status, "unavailable");
    assert_eq!(missing.ready_replicas, None);
    live.desired_replicas = 0;
    live.ready_replicas = 0;
    original.target_replicas = 0;
    assert_eq!(original.view(Some(&live), true).status, "disabled");
}

#[test]
fn ordinary_and_inventory_only_users_cannot_manage_pools() {
    for capability in [
        Capability::UseOwnSandboxes,
        Capability::ReadRuntimeInventory,
    ] {
        let caller = Caller::new(
            layer2_core::SubjectId("user".into()),
            layer2_core::Authorization::new([capability]),
        );
        assert_eq!(authorize(&caller), Err(WarmPoolError::Forbidden));
    }
}

#[path = "mongo_tests.rs"]
mod mongo;

#[test]
fn deployment_owned_image_admits_native_cold_and_warm_contracts() {
    let original = target().plan.spec;
    let image = deployment_image(original.image.uri.clone());
    layer3_adapter::runtime_bootstrap::admit_pool_image(&image).unwrap();
    let mut cold = original.clone();
    cold.image = image.clone();
    cold.home.mode = layer3_adapter::HomeVolumeMode::NativeVctEmptyV1;
    cold.home.template_name.clear();
    cold.home.template_generation.clear();
    cold.validate().unwrap();
    assert_eq!(
        image, original.image,
        "cold and warm templates share image identity"
    );
    let deployment = layer3_adapter::hosted::HostedDeployment {
        image,
        ..Default::default()
    };
    deployment.contract().unwrap();
}
