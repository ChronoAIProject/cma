use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::hosted::{
    HostedAccessToolBinding, HostedAccessToolSource, HostedProviderToolBinding,
    HostedProviderToolSource,
};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

pub(in crate::hosted) struct Source {
    access: HostedAccessToolBinding,
    provider: HostedProviderToolBinding,
    fault: u8,
    foreign: Option<IngressOwnershipPolicy>,
    preparations: AtomicUsize,
    pub(in crate::hosted) confirmations: AtomicUsize,
}
impl Source {
    fn prepare<'a, T: Clone + Send + Sync + 'a>(
        &'a self,
        value: &'a T,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, T>, AdmittedError> {
        AdmittedOperation::new(
            self.foreign.as_ref().unwrap_or(policy),
            IngressClass::Data,
            AllocationPlan::checked("synthetic transport binding", &[16 * 1024])?,
            move |_| async move {
                self.preparations.fetch_add(1, SeqCst);
                match self.fault {
                    1 => return Err(AdapterError::Rejected("authority refused".into())),
                    2 => return Err(AdapterError::RuntimeLedgerHold("e".repeat(2048))),
                    3 => std::future::pending::<()>().await,
                    _ => {}
                }
                Ok(value.clone())
            },
        )
    }
    fn confirm<'a>(
        &'a self,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ()>, AdmittedError> {
        AdmittedOperation::new(
            policy,
            IngressClass::Data,
            AllocationPlan::checked("synthetic tool confirmation", &[2048])?,
            move |_| async move {
                self.confirmations.fetch_add(1, SeqCst);
                if self.fault == 4 {
                    std::future::pending::<()>().await;
                }
                if self.fault == 5 {
                    return Err(AdapterError::Unavailable("confirmation unknown".into()));
                }
                Ok(())
            },
        )
    }
}
#[async_trait::async_trait]
impl HostedAccessToolSource for Source {
    fn prepare_transport_admitted<'a>(
        &'a self,
        binding: &'a HostedAccessToolBinding,
        origin: &'a str,
        session: &'a str,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, HostedAccessToolBinding>, AdmittedError> {
        assert_eq!(binding, &self.access);
        assert_eq!(
            (origin, session, sequence),
            ("https://new.example.test", "conversation", 1)
        );
        self.prepare(&self.access, policy)
    }
    fn confirm_transport_admitted<'a>(
        &'a self,
        binding: &'a HostedAccessToolBinding,
        session: &'a str,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ()>, AdmittedError> {
        assert_eq!(binding, &self.access);
        assert_eq!((session, sequence), ("conversation", 1));
        self.confirm(policy)
    }
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
        assert_eq!(self.fault, 6, "legacy access preparation");
        Ok(self.access.clone())
    }
    async fn confirm_transport(
        &self,
        _: &HostedAccessToolBinding,
        _: &str,
        _: u64,
    ) -> crate::Result<()> {
        assert_eq!(self.fault, 6, "legacy access confirmation");
        Ok(())
    }
}
#[async_trait::async_trait]
impl HostedProviderToolSource for Source {
    fn prepare_transport_admitted<'a>(
        &'a self,
        binding: &'a HostedProviderToolBinding,
        origin: &'a str,
        session: &'a str,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, HostedProviderToolBinding>, AdmittedError> {
        assert_eq!(binding, &self.provider);
        assert_eq!(
            (origin, session, sequence),
            ("https://new.example.test", "conversation", 1)
        );
        self.prepare(&self.provider, policy)
    }
    fn confirm_transport_admitted<'a>(
        &'a self,
        binding: &'a HostedProviderToolBinding,
        session: &'a str,
        sequence: u64,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ()>, AdmittedError> {
        assert_eq!(binding, &self.provider);
        assert_eq!((session, sequence), ("conversation", 1));
        self.confirm(policy)
    }
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
        assert_eq!(self.fault, 6, "legacy provider preparation");
        Ok(self.provider.clone())
    }
    async fn confirm_transport(
        &self,
        _: &HostedProviderToolBinding,
        _: &str,
        _: u64,
    ) -> crate::Result<()> {
        assert_eq!(self.fault, 6, "legacy provider confirmation");
        Ok(())
    }
}

fn binding(
    provider: bool,
    fault: u8,
    foreign: Option<IngressOwnershipPolicy>,
) -> (HostedContextRefresh, Arc<Source>) {
    let (binding, _) = super::super::writer_tests::binding(0);
    attach(binding, provider, fault, foreign)
}

pub(in crate::hosted) fn attach(
    mut binding: HostedContextRefresh,
    provider: bool,
    fault: u8,
    foreign: Option<IngressOwnershipPolicy>,
) -> (HostedContextRefresh, Arc<Source>) {
    let request = crate::AgentOpenRequest::new(
        "synthetic-open",
        crate::RuntimeOptions {
            slot: crate::AgentSlot::new("synthetic-agent"),
            ..Default::default()
        },
        None,
    );
    let runtime = RuntimeHandle("synthetic-runtime".into());
    let source = Arc::new(Source {
        access: HostedAccessToolBinding::new(
            "https://new.example.test",
            crate::RuntimeCredential::new(format!("cma_at_{}", "a".repeat(64))),
            runtime.clone(),
            &request,
            1,
            Some("conversation".into()),
        )
        .unwrap(),
        provider: HostedProviderToolBinding::new(
            "a".repeat(64),
            runtime,
            request,
            1,
            crate::ToolProxyRequest {
                sandbox_id: "synthetic-sandbox".into(),
                generation: 1,
                endpoint: "https://gateway.example.test/mcp".into(),
                credential_generation: 1,
            },
            None,
        )
        .unwrap(),
        fault,
        foreign,
        preparations: AtomicUsize::new(0),
        confirmations: AtomicUsize::new(0),
    });
    let tool = if provider {
        source.provider.acp_server()
    } else {
        source.access.acp_server()
    };
    Arc::get_mut(&mut binding.0).unwrap().tools = vec![tool];
    let binding = if provider {
        binding.with_provider(source.clone(), source.provider.clone())
    } else {
        binding.with_access(source.clone(), source.access.clone())
    };
    drop(binding.0.installed_transport.write().unwrap());
    (binding, source)
}

#[test]
fn admitted_tool_binding_is_shared_through_installation_and_repeated_preparation() {
    if deallocation::isolated("hosted::managed_context::transport_admission::tests::admitted_tool_binding_is_shared_through_installation_and_repeated_preparation") { return; }
    for provider in [false, true] {
        let (root, policy) = super::super::guidance_admission::fixture::policy();
        let (binding, source) = binding(provider, 0, None);
        let mut probe = AllocationProbe::new();
        let prepared = probe.track(|| {
            binding
                .prepare_transport_admitted(
                    "conversation",
                    1,
                    Some("https://new.example.test"),
                    &policy,
                )
                .now_or_never()
                .unwrap()
                .unwrap()
                .unwrap()
        });
        let charged = root.reserved_work_bytes();
        assert!(probe.peak() <= charged);
        let cloned = probe.track(|| prepared.clone_admitted(&policy).unwrap());
        assert_eq!(root.reserved_work_bytes(), charged);
        probe.track(|| {
            binding
                .confirm_transport_admitted(&prepared, "conversation", 1, &policy)
                .now_or_never()
                .unwrap()
                .unwrap()
        });
        drop((prepared, cloned));
        assert_eq!(root.reserved_work_bytes(), charged);
        assert!(probe.live() > 0);
        assert!(probe.live() <= charged);
        let frame = binding.tools_frame(Some(&policy)).unwrap();
        assert_eq!(
            frame[0]["name"],
            if provider { "cmaeg" } else { "cma_access" }
        );
        drop(frame);
        let next = binding
            .prepare_transport_admitted(
                "conversation",
                1,
                Some("https://new.example.test"),
                &policy,
            )
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(source.preparations.load(SeqCst), 2);
        assert_eq!(source.confirmations.load(SeqCst), 1);
        drop(next);
        drop((binding, source));
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }
}

#[test]
fn admitted_tool_failures_keep_categories_and_never_install_unconfirmed_bindings() {
    for provider in [false, true] {
        for fault in [1, 2, 5] {
            let (root, policy) = super::super::guidance_admission::fixture::policy();
            let (binding, source) = binding(provider, fault, None);
            let prepared = binding
                .prepare_transport_admitted(
                    "conversation",
                    1,
                    Some("https://new.example.test"),
                    &policy,
                )
                .now_or_never()
                .unwrap();
            let error = if fault == 5 {
                let prepared = prepared.unwrap().unwrap();
                binding
                    .confirm_transport_admitted(&prepared, "conversation", 1, &policy)
                    .now_or_never()
                    .unwrap()
                    .unwrap_err()
            } else {
                prepared.err().unwrap()
            };
            assert!(binding.0.installed_transport.read().unwrap().is_none());
            match fault {
                1 => assert!(matches!(error, AdmittedError::Unsupported(_))),
                2 => assert!(matches!(
                    error.as_adapter(),
                    Some(AdapterError::RuntimeLedgerHold(_))
                )),
                5 => assert!(matches!(
                    error.as_adapter(),
                    Some(AdapterError::Unavailable(_))
                )),
                _ => unreachable!(),
            }
            let clone = error.clone();
            drop((binding, source, error));
            assert_eq!(root.reserved_work_bytes(), clone.requested_bytes());
            drop(clone);
            root.assert_idle();
        }
    }
}

#[test]
fn admitted_tool_foreign_sources_and_cancelled_work_release_without_installation() {
    for provider in [false, true] {
        let (root, policy) = super::super::guidance_admission::fixture::policy();
        let (foreign_root, foreign) = super::super::guidance_admission::fixture::policy();
        let (binding, source) = binding(provider, 0, Some(foreign));
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
        assert!(error.refusal().is_some());
        assert_eq!(source.preparations.load(SeqCst), 0);
        foreign_root.assert_idle();
        for fault in [3, 4] {
            let (binding, source) = self::binding(provider, fault, None);
            if fault == 3 {
                let mut operation = Box::pin(binding.prepare_transport_admitted(
                    "conversation",
                    1,
                    Some("https://new.example.test"),
                    &policy,
                ));
                assert!(operation.as_mut().now_or_never().is_none());
                drop(operation);
            } else {
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
                let mut operation = Box::pin(binding.confirm_transport_admitted(
                    &prepared,
                    "conversation",
                    1,
                    &policy,
                ));
                assert!(operation.as_mut().now_or_never().is_none());
                drop(operation);
                drop(prepared);
            }
            assert_eq!(source.preparations.load(SeqCst), 1);
            assert!(binding.0.installed_transport.read().unwrap().is_none());
            root.assert_idle();
        }
    }
}

#[path = "transport_refusal_tests.rs"]
mod refusal_tests;

#[path = "transport_snapshot_tests.rs"]
mod snapshot_tests;
