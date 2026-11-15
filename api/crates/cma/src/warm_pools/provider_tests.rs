use super::*;

impl Provider {
    pub(super) fn binding(&self) -> NativePoolBinding {
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
        _: &[layer3_adapter::HomeVolumeAttachment],
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
    async fn resize_fenced(
        &self,
        binding: &NativePoolBinding,
        target: &layer3_adapter::agent_sandbox::NativePoolTarget,
    ) -> layer3_adapter::Result<NativePoolStatus> {
        self.calls.lock().unwrap().push(format!(
            "fenced:{}:{}:{}",
            target.target_id, target.revision, target.replicas
        ));
        if !self.supports_target_fencing {
            return Err(layer3_adapter::AdapterError::Unsupported(
                "target watermark unavailable".into(),
            ));
        }
        assert_eq!(binding, &self.binding());
        Ok(NativePoolStatus {
            binding: binding.clone(),
            desired_replicas: target.replicas,
            ready_replicas: target.replicas,
            observed: true,
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
