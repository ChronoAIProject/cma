use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressWorkScope,
};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(in crate::hosted) fn plan(bytes: usize) -> AllocationPlan {
    AllocationPlan::checked("backend ingress fixture", &[bytes]).unwrap()
}

pub(in crate::hosted) fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(1024 * 1024);
    let bounds = ClassBounds::new(root.capacity_bytes(), plan(64 * 1024), plan(64 * 1024))
        .unwrap()
        .with_attachment_limit(2)
        .unwrap();
    let policy = IngressOwnershipPolicy::new(root.clone(), bounds).unwrap();
    // Cold class-lock storage has its own accounted-construction tests. These
    // probes isolate whether forwarding adds a future or payload allocation.
    for class in [
        IngressClass::Data,
        IngressClass::Control,
        IngressClass::Transformation,
    ] {
        drop(policy.admit(class, plan(1)).unwrap());
    }
    (root, policy)
}

pub(in crate::hosted) fn request(policy: &IngressOwnershipPolicy) -> OwnedWork<RequestFrame> {
    IngressWorkScope::new(policy.admit(IngressClass::Data, plan(4096)).unwrap()).initialize_owned(
        || {
            RequestFrame::RuntimeStatus(crate::RuntimeStatusRequest {
                sandbox_id: "a".repeat(64),
                generation: 1,
                home_identity: "home".into(),
            })
        },
    )
}

#[derive(Default)]
pub(in crate::hosted) struct Transport {
    pub calls: AtomicUsize,
    pub started: AtomicUsize,
    pub cancelled: AtomicUsize,
    pub bytes: AtomicUsize,
    pub pending: bool,
    pub error: Option<AdmittedError>,
    legacy: LegacyTransport,
}

struct Cancelled<'a>(&'a AtomicUsize);
impl Drop for Cancelled<'_> {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl HostedControlTransport for Transport {
    fn exchange_admitted<'a>(
        &'a self,
        _: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        assert!(request.belongs_to(policy));
        self.calls.fetch_add(1, Ordering::SeqCst);
        let operation = AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            plan(0),
            move |_| async move {
                self.started.fetch_add(1, Ordering::SeqCst);
                if self.pending {
                    let _cancelled = Cancelled(&self.cancelled);
                    std::future::pending::<()>().await;
                }
                if let Some(error) = &self.error {
                    return Err(error.clone());
                }
                Ok(request.map(|request| {
                    let RequestFrame::RuntimeStatus(request) = request else {
                        panic!("expected owned status request")
                    };
                    ResponseFrame::CacheSkillArchive {
                        sha256: request.sandbox_id,
                    }
                }))
            },
        )?;
        self.bytes
            .store(operation.requested_bytes(), Ordering::SeqCst);
        Ok(operation)
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> crate::Result<ResponseFrame> {
        self.legacy.exchange(handle, request).await
    }

    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("exchange cannot open an agent")
    }

    async fn retire(&self, _: &RuntimeHandle) -> crate::Result<()> {
        panic!("exchange cannot retire a runtime")
    }
}

#[derive(Default)]
struct LegacyTransport(AtomicUsize);

#[async_trait::async_trait]
impl HostedControlTransport for LegacyTransport {
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(ResponseFrame::CacheSkillArchive {
            sha256: "legacy".into(),
        })
    }

    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("exchange cannot open an agent")
    }

    async fn retire(&self, _: &RuntimeHandle) -> crate::Result<()> {
        panic!("exchange cannot retire a runtime")
    }
}

pub(in crate::hosted) fn backend(
    transport: Arc<dyn HostedControlTransport>,
) -> ProviderHostedBackend {
    ProviderHostedBackend::new(
        SandboxClient::new("http://unused.invalid", uuid::Uuid::new_v4().to_string()),
        transport,
        "fixture",
    )
    .unwrap()
}

#[test]
fn b6_ingress_transport_default_refuses_without_allocating_a_legacy_future() {
    let (root, policy) = policy();
    let transport = Arc::new(LegacyTransport::default());
    let backend = backend(transport.clone());
    let handle = RuntimeHandle::new("provider");
    let request = request(&policy);
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| backend.exchange_admitted(&handle, request, &policy))
        .unwrap_err();
    assert!(matches!(error, AdmittedError::Unsupported(_)));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(transport.0.load(Ordering::SeqCst), 0);
    root.assert_idle();
}

#[test]
fn b6_ingress_provider_rejects_foreign_root_attachment_and_unowned_inputs_before_dispatch() {
    let (root, policy) = policy();
    let (foreign_root, foreign) = self::policy();
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let transport = Arc::new(Transport::default());
    let backend = backend(transport.clone());
    let handle = RuntimeHandle::new("provider");
    for request in [
        request(&foreign),
        request(second.policy()),
        IngressWorkScope::inherit_owned(async { RequestFrame::Resources })
            .now_or_never()
            .unwrap(),
    ] {
        let mut probe = AllocationProbe::new();
        let error = probe
            .track(|| backend.exchange_admitted(&handle, request, first.policy()))
            .unwrap_err();
        assert!(matches!(
            error.refusal(),
            Some(AdmissionRefusal::InvalidPlan { .. })
        ));
        assert_eq!(probe.allocated(), 0);
    }
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    drop((first, second));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[test]
fn b6_ingress_provider_forwards_one_box_and_retains_payload_and_attachment() {
    let (root, policy) = policy();
    let attachment = policy.admit_attachment().unwrap();
    let blocked_slot = policy.admit_attachment().unwrap();
    let source = request(attachment.policy());
    let RequestFrame::RuntimeStatus(input) = &*source else {
        unreachable!()
    };
    let address = input.sandbox_id.as_ptr();
    let transport = Arc::new(Transport::default());
    let backend = backend(transport.clone());
    let handle = RuntimeHandle::new("provider");
    let before = root.reserved_work_bytes();
    let mut operation = backend
        .exchange_admitted(&handle, source, attachment.policy())
        .unwrap();
    assert_eq!(
        root.reserved_work_bytes() - before,
        transport.bytes.load(Ordering::SeqCst)
    );
    assert_eq!(
        operation.requested_bytes(),
        transport.bytes.load(Ordering::SeqCst)
    );
    assert_eq!(transport.started.load(Ordering::SeqCst), 0);
    let response = (&mut operation).now_or_never().unwrap().unwrap();
    assert!(response.belongs_to(attachment.policy()));
    let ResponseFrame::CacheSkillArchive { sha256 } = &*response else {
        unreachable!()
    };
    assert_eq!(sha256.as_ptr(), address);
    assert_eq!(sha256.len(), 64);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    drop(operation);
    drop((attachment, backend, transport));
    assert!(policy.admit_attachment().is_err());
    let shared = response.into_shared_admitted().unwrap();
    let last = shared.clone();
    drop(shared);
    assert!(policy.admit_attachment().is_err());
    drop(last);
    drop(policy.admit_attachment().unwrap());
    drop(blocked_slot);
    root.assert_idle();
}

#[test]
fn b6_ingress_provider_preserves_owned_error_identity_and_lifetime() {
    let (root, policy) = policy();
    let scope = IngressWorkScope::new(policy.admit(IngressClass::Data, plan(4096)).unwrap());
    let original = AdmittedError::Adapter(
        scope.initialize(|| AdapterError::Rejected("synthetic refusal".into())),
    );
    let address = original.as_adapter().unwrap() as *const AdapterError;
    let transport = Arc::new(Transport {
        error: Some(original.clone()),
        ..Default::default()
    });
    let backend = backend(transport.clone());
    let handle = RuntimeHandle::new("provider");
    let returned = backend
        .exchange_admitted(&handle, request(&policy), &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        returned.as_adapter().unwrap() as *const AdapterError,
        address
    );
    drop((original, scope, backend, transport));
    assert_eq!(root.reserved_work_bytes(), 4096);
    drop(returned);
    root.assert_idle();
}

#[test]
fn b6_ingress_provider_capacity_refusal_precedes_future_allocation_and_poll() {
    let (root, policy) = policy();
    let transport = Arc::new(Transport::default());
    let backend = backend(transport.clone());
    let handle = RuntimeHandle::new("provider");
    let source = request(&policy);
    let pressure = policy
        .admit(
            IngressClass::Data,
            plan(policy.budget(IngressClass::Data).available_bytes()),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| backend.exchange_admitted(&handle, source, &policy))
        .unwrap_err();
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(transport.started.load(Ordering::SeqCst), 0);
    drop(pressure);
    root.assert_idle();
}

#[test]
fn b6_ingress_provider_cancellation_keeps_input_owned_until_operation_drop() {
    let (root, policy) = policy();
    for polled in [false, true] {
        let attachment = policy.admit_attachment().unwrap();
        let blocked_slot = policy.admit_attachment().unwrap();
        let transport = Arc::new(Transport {
            pending: true,
            ..Default::default()
        });
        let backend = backend(transport.clone());
        let handle = RuntimeHandle::new("provider");
        let mut operation = backend
            .exchange_admitted(&handle, request(attachment.policy()), attachment.policy())
            .unwrap();
        if polled {
            assert!((&mut operation).now_or_never().is_none());
        }
        drop(attachment);
        assert!(policy.admit_attachment().is_err());
        drop(operation);
        assert_eq!(
            transport.cancelled.load(Ordering::SeqCst),
            usize::from(polled)
        );
        drop(policy.admit_attachment().unwrap());
        drop(blocked_slot);
        root.assert_idle();
    }
}

#[test]
fn b6_ingress_provider_legacy_exchange_and_recovery_capabilities_are_unchanged() {
    let transport = Arc::new(Transport::default());
    let backend = backend(transport.clone());
    let handle = RuntimeHandle::new("provider");
    assert_eq!(
        backend
            .exchange(&handle, RequestFrame::Resources)
            .now_or_never()
            .unwrap()
            .unwrap(),
        ResponseFrame::CacheSkillArchive {
            sha256: "legacy".into()
        }
    );
    assert_eq!(transport.legacy.0.load(Ordering::SeqCst), 1);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    let command = crate::hosted::RuntimeRecoveryCommand {
        program: "/fixture".into(),
        args: vec![],
        input: vec![],
    };
    assert!(matches!(
        backend
            .run_recovery_command(&handle, &command)
            .now_or_never()
            .unwrap(),
        Err(AdapterError::Unsupported(_))
    ));
    let (root, policy) = policy();
    let command = IngressWorkScope::new(policy.admit(IngressClass::Data, plan(4096)).unwrap())
        .initialize_owned(|| command);
    assert!(matches!(
        backend.run_recovery_command_admitted(&handle, command, &policy),
        Err(AdmittedError::Unsupported(_))
    ));
    root.assert_idle();
}

#[test]
fn b6_ingress_provider_forwarding_allocates_only_the_child_operation() {
    let (root, policy) = policy();
    let transport = Arc::new(Transport::default());
    let backend = backend(transport.clone());
    let handle = RuntimeHandle::new("provider");
    let mut totals = Vec::new();
    for forwarded in [false, true] {
        let source = request(&policy);
        let mut probe = AllocationProbe::new();
        let operation = probe.track(|| {
            if forwarded {
                backend.exchange_admitted(&handle, source, &policy)
            } else {
                transport.exchange_admitted(&handle, source, &policy)
            }
            .unwrap()
        });
        totals.push(probe.allocated());
        assert!(probe.peak() <= operation.requested_bytes());
        drop(operation);
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }
    assert!(totals[0] > 0);
    assert_eq!(totals[0], totals[1]);
}
