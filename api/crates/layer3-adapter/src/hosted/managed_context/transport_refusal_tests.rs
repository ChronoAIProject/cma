use super::*;

struct Legacy;
#[async_trait::async_trait]
impl HostedAccessToolSource for Legacy {
    async fn resolve(
        &self,
        _: &RuntimeHandle,
        _: &crate::AgentOpenRequest,
        _: u64,
    ) -> crate::Result<Option<HostedAccessToolBinding>> {
        unreachable!()
    }
    async fn confirm_open(&self, _: &HostedAccessToolBinding, _: &str) -> crate::Result<()> {
        unreachable!()
    }
    async fn prepare_transport(
        &self,
        _: &HostedAccessToolBinding,
        _: &str,
        _: &str,
        _: u64,
    ) -> crate::Result<HostedAccessToolBinding> {
        panic!("legacy preparation must not be boxed or invoked")
    }
    async fn confirm_transport(
        &self,
        _: &HostedAccessToolBinding,
        _: &str,
        _: u64,
    ) -> crate::Result<()> {
        panic!("legacy confirmation must not be boxed or invoked")
    }
}
#[async_trait::async_trait]
impl HostedProviderToolSource for Legacy {
    async fn resolve(
        &self,
        _: &RuntimeHandle,
        _: &crate::AgentOpenRequest,
        _: u64,
    ) -> crate::Result<Option<HostedProviderToolBinding>> {
        unreachable!()
    }
    async fn confirm_open(&self, _: &HostedProviderToolBinding, _: &str) -> crate::Result<()> {
        unreachable!()
    }
    async fn prepare_transport(
        &self,
        _: &HostedProviderToolBinding,
        _: &str,
        _: &str,
        _: u64,
    ) -> crate::Result<HostedProviderToolBinding> {
        panic!("legacy preparation must not be boxed or invoked")
    }
    async fn confirm_transport(
        &self,
        _: &HostedProviderToolBinding,
        _: &str,
        _: u64,
    ) -> crate::Result<()> {
        panic!("legacy confirmation must not be boxed or invoked")
    }
}

#[test]
fn admitted_transport_refuses_legacy_sources_before_invocation_and_before_installation() {
    for provider in [false, true] {
        let (root, policy) = super::super::super::guidance_admission::fixture::policy();
        let (mut binding, _) = binding(provider, 0, None);
        let prepared = binding
            .prepare_transport_admitted(
                "conversation",
                1,
                Some("https://new.example.test"),
                &policy,
            )
            .now_or_never()
            .unwrap()
            .unwrap()
            .unwrap();
        let state = Arc::get_mut(&mut binding.0).unwrap();
        if provider {
            state.provider.as_mut().unwrap().0 = Arc::new(Legacy);
        } else {
            state.access.as_mut().unwrap().0 = Arc::new(Legacy);
        }
        assert!(binding
            .prepare_transport_admitted("conversation", 1, None, &policy)
            .now_or_never()
            .unwrap()
            .unwrap()
            .is_none());
        let error = binding
            .prepare_transport_admitted(
                "conversation",
                1,
                Some("https://new.example.test"),
                &policy,
            )
            .now_or_never()
            .unwrap()
            .err()
            .unwrap();
        assert!(matches!(error, AdmittedError::Unsupported(_)));
        let error = binding
            .confirm_transport_admitted(&prepared, "conversation", 1, &policy)
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, AdmittedError::Unsupported(_)));
        assert!(binding.0.installed_transport.read().unwrap().is_none());
        drop(prepared);
        root.assert_idle();
    }
}

#[test]
fn admitted_transport_refuses_pressure_before_copying_source_bindings() {
    for provider in [false, true] {
        let (root, policy) = super::super::super::guidance_admission::fixture::policy();
        let (binding, source) = binding(provider, 0, None);
        let pressure = policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "tool preparation pressure",
                    &[policy.budget(IngressClass::Data).available_bytes()],
                )
                .unwrap(),
            )
            .unwrap();
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| {
            binding
                .prepare_transport_admitted(
                    "conversation",
                    1,
                    Some("https://new.example.test"),
                    &policy,
                )
                .now_or_never()
                .unwrap()
                .err()
                .unwrap()
        });
        assert!(error.refusal().is_some());
        assert_eq!(source.preparations.load(SeqCst), 0);
        assert_eq!(probe.allocated(), 0);
        drop(pressure);
        root.assert_idle();
    }
}
