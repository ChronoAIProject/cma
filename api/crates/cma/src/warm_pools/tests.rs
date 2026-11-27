use super::*;
use layer3_adapter::agent_sandbox::{
    NativePoolClaim, NativePoolClaimRequest, NativePoolCleanup, NativePoolControl,
    NativePoolPodPolicy,
};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

pub(crate) fn target() -> PoolTarget {
    let spec = NativePoolSpec {
        cold_canary: false,
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
        activated_at: None,
        machine_plan_activated_at: None,
        machine: false,
        retiring: false,
        retired: false,
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
                template_network_policy: None,
                fs_group_change_policy: None,
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
        operator_replicas: None,
    }
}

struct Memory {
    target: Mutex<PoolTarget>,
    bind_response_loss: AtomicBool,
    retirement_block: Mutex<std::collections::VecDeque<bool>>,
    drain_block: Mutex<std::collections::VecDeque<bool>>,
    calls: Arc<Mutex<Vec<String>>>,
}
#[async_trait]
impl PoolTargetStore for Memory {
    async fn retirement_drain_homes(
        &self,
        _: &PoolTarget,
        _: &str,
    ) -> Result<Option<NativePoolDrain>, WarmPoolError> {
        self.calls.lock().unwrap().push("drain-scan".into());
        Ok((!self
            .drain_block
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(false))
        .then_some(NativePoolDrain::default()))
    }
    async fn retirement_homes(
        &self,
        _: &PoolTarget,
        _: &str,
    ) -> Result<Option<Vec<layer3_adapter::HomeVolumeAttachment>>, WarmPoolError> {
        self.calls.lock().unwrap().push("scan".into());
        Ok((!self
            .retirement_block
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(false))
        .then_some(vec![]))
    }
    async fn begin_retirement(&self, _: &PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        self.calls.lock().unwrap().push("persist-retiring".into());
        let mut target = self.target.lock().unwrap();
        if !target.retiring {
            target.retiring = true;
            target.target_replicas = 0;
            target.revision += 1;
        }
        Ok(target.clone())
    }
    async fn remove_retired(&self, _: &PoolTarget) -> Result<(), WarmPoolError> {
        self.calls.lock().unwrap().push("remove-target".into());
        Ok(())
    }
    async fn activate(&self, _: &str) -> Result<(), WarmPoolError> {
        Ok(())
    }
    async fn list(&self) -> Result<Vec<PoolTarget>, WarmPoolError> {
        Ok(vec![self.target.lock().unwrap().clone()])
    }
    async fn find(&self, _: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        Ok(Some(self.target.lock().unwrap().clone()))
    }
    async fn insert(&self, _: PoolTarget) -> Result<PoolTarget, WarmPoolError> {
        unreachable!()
    }
    async fn resize(
        &self,
        id: &str,
        body: &ResizeWarmPoolBody,
        operator_choice: bool,
    ) -> Result<PoolTarget, WarmPoolError> {
        self.calls.lock().unwrap().push(format!(
            "store-resize:{id}:{}:{}{}",
            body.expected_revision,
            body.target_replicas,
            if operator_choice { ":choice" } else { "" },
        ));
        let mut target = self.target.lock().unwrap();
        assert_eq!(id, target.id);
        if body.expected_revision != target.revision as u64 {
            return Err(WarmPoolError::Conflict);
        }
        target.target_replicas = body.target_replicas;
        target.revision += 1;
        Ok(target.clone())
    }
    async fn claim(&self) -> Result<Option<PoolTarget>, WarmPoolError> {
        Ok(Some(self.target.lock().unwrap().clone()))
    }
    async fn claim_target(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.calls.lock().unwrap().push(format!("claim:{id}"));
        let target = self.target.lock().unwrap();
        Ok((target.id == id && !target.retiring && !target.retired).then(|| target.clone()))
    }
    async fn claim_retirement(&self, id: &str) -> Result<Option<PoolTarget>, WarmPoolError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("claim-retire:{id}"));
        let target = self.target.lock().unwrap();
        Ok((target.id == id && !target.retired).then(|| target.clone()))
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
        let mut target = self.target.lock().unwrap();
        target.error = (!success).then(|| "unavailable".into());
        if success {
            target.applied_revision = target.revision;
        }
        Ok(())
    }
}

struct Provider {
    calls: Arc<Mutex<Vec<String>>>,
    original: NativePoolConfiguration,
    refuse_replaced_uid: AtomicBool,
    retired: AtomicBool,
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
    async fn drain_pool(
        &self,
        binding: &NativePoolBinding,
        _: &NativePoolDrain,
    ) -> layer3_adapter::Result<()> {
        self.calls.lock().unwrap().push("prove-drain".into());
        self.resize(binding, 0).await.map(|_| ())
    }
    async fn begin_retirement(&self, _: &NativePoolBinding) -> layer3_adapter::Result<()> {
        self.calls.lock().unwrap().push("mark-provider".into());
        Ok(())
    }
    async fn retire_pool(
        &self,
        _: &NativePoolBinding,
        _: &[layer3_adapter::HomeVolumeAttachment],
    ) -> layer3_adapter::Result<bool> {
        self.calls.lock().unwrap().push("prove-retirement".into());
        Ok(self.retired.load(Ordering::SeqCst))
    }
    fn plan(&self, spec: &NativePoolSpec) -> layer3_adapter::Result<NativePoolConfiguration> {
        assert_eq!(spec, &self.original.spec);
        Ok(self.original.clone())
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
        self.calls.lock().unwrap().push("inventory".into());
        Ok(vec![NativePoolStatus {
            binding: self.binding(),
            desired_replicas: 3,
            ready_replicas: 1,
            observed: true,
        }])
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
        retired: AtomicBool::new(false),
    };
    let store = Memory {
        target: Mutex::new(original),
        bind_response_loss: AtomicBool::new(true),
        retirement_block: Mutex::new(std::collections::VecDeque::new()),
        drain_block: Mutex::new(std::collections::VecDeque::new()),
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
fn default_target_is_stable_and_pins_exact_plan_and_capacity() {
    let original = target();
    let body = ConfigureWarmPoolBody {
        cpu_millis: original.requested.cpu_millis,
        memory_mib: original.requested.memory_mib,
        target_replicas: 1,
    };
    let first = default_target(original.plan.clone(), body.clone());
    let second = default_target(original.plan.clone(), body);
    assert_eq!(first.id, second.id);
    assert_eq!(first.actor, SYSTEM_ACTOR);
    assert_eq!(first.target_replicas, 1);
    assert_eq!(first.requested.cpu_millis, original.plan.spec.cpu_millis);
    assert_eq!(first.requested.memory_mib, original.plan.spec.memory_mib);
    assert_eq!(first.plan, original.plan);
    assert!(first.binding.is_none());
    assert_eq!(first.applied_revision, 0);
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
fn inventory_identifies_original_and_current_default_policy_even_with_identical_resources() {
    let original = target();
    let mut current = original.clone();
    current.plan.pod_policy.fs_group_change_policy =
        Some(layer3_adapter::agent_sandbox::NativeFsGroupChangePolicy::OnRootMismatch);
    current.plan.config_sha256 = "b".repeat(64);
    let provider = Provider {
        calls: Arc::default(),
        original: current.plan.clone(),
        refuse_replaced_uid: AtomicBool::new(false),
        retired: AtomicBool::new(false),
    };
    let spec = &current.plan.spec;
    let capacity = layer3_adapter::SandboxCapacity {
        cpu_millis: spec.cpu_millis,
        memory_mib: spec.memory_mib,
    };
    let default = default_config_sha256(Some(&provider), &spec.image, Some(&spec.home), &capacity);
    assert!(!current::current_plan(&provider, &spec.image, &spec.home, &original).unwrap());
    assert!(current::current_plan(&provider, &spec.image, &spec.home, &current).unwrap());
    let mut next_image = spec.image.clone();
    next_image.uri = format!("registry.example/runtime@sha256:{}", "c".repeat(64));
    assert!(!current::current_plan(&provider, &next_image, &spec.home, &current).unwrap());
    let mut next_home = spec.home.clone();
    next_home.capacity_gib += 1;
    assert!(!current::current_plan(&provider, &spec.image, &next_home, &current).unwrap());
    assert_eq!(
        original.view(None, true).image,
        current.view(None, true).image
    );
    assert_eq!(
        original.view(None, true).cpu_millis,
        current.view(None, true).cpu_millis
    );
    assert_eq!(
        original.view(None, true).memory_mib,
        current.view(None, true).memory_mib
    );
    assert_ne!(Some(original.view(None, true).config_sha256), default);
    assert_eq!(Some(current.view(None, true).config_sha256), default);
    assert_eq!(
        default_config_sha256(None, &spec.image, Some(&spec.home), &capacity),
        None
    );
    assert_eq!(
        default_config_sha256(Some(&provider), &spec.image, None, &capacity),
        None
    );
    assert_eq!(
        replay(original.clone(), &original.requested)
            .unwrap()
            .view(None, true)
            .config_sha256,
        original.plan.config_sha256
    );
}

#[path = "view_tests.rs"]
mod view;

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
    cold.home.mode = layer3_adapter::HomeVolumeMode::NativeVctFullHomeV2;
    cold.home.template_name.clear();
    cold.home.template_generation.clear();
    cold.validate().unwrap();
    assert_eq!(image.uri, original.image.uri);
    assert_eq!(
        image.labels[layer3_adapter::runtime_bootstrap::RUNTIME_HOME_LAYOUT_LABEL],
        "2"
    );
    // Historical pool metadata remains valid without the v2 capability.
    cold.home.mode = layer3_adapter::HomeVolumeMode::NativeVctEmptyV1;
    cold.image = original.image;
    cold.validate().unwrap();
    let deployment = layer3_adapter::hosted::HostedDeployment {
        image,
        ..Default::default()
    };
    deployment.contract().unwrap();
}

#[path = "resize_tests.rs"]
mod resize;

#[path = "retirement_tests.rs"]
mod retirement_tests;

#[path = "startup_tests.rs"]
pub(crate) mod startup_tests;

#[path = "runtime_pointer_tests.rs"]
pub(crate) mod runtime_pointer;
