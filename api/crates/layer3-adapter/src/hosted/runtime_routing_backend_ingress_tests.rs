use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::hosted::provider_backend::ingress_tests::{backend, plan, policy, request, Transport};
use crate::ingress_budget::{AdmissionRefusal, IngressClass, IngressWorkScope};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Routes;
#[async_trait::async_trait]
impl RuntimeRoutingStore for Routes {
    async fn select(&self, _: &str, _: u64, _: bool) -> Result<bool> {
        panic!("existing runtime exchange cannot choose a new allocation route")
    }
}

fn router(
    native: Option<Arc<dyn HostedBackend>>,
    provider: Option<Arc<dyn HostedBackend>>,
) -> RuntimeBackends {
    RuntimeBackends {
        agent_sandbox: native,
        opensandbox: provider,
        allocate_agent_sandbox: true,
        routing: Arc::new(Routes),
        home_routes: None,
    }
}

fn handles() -> [RuntimeHandle; 2] {
    [
        RuntimeHandle::new("agent-sandbox:fixture:uid"),
        RuntimeHandle::new("provider"),
    ]
}

#[test]
fn b6_ingress_router_keeps_recorded_provider_and_the_child_operation() {
    let (root, policy) = policy();
    let native = Arc::new(Transport::default());
    let provider = Arc::new(Transport::default());
    let mut router = router(
        Some(Arc::new(backend(native.clone()))),
        Some(Arc::new(backend(provider.clone()))),
    );
    for default in [false, true] {
        router.allocate_agent_sandbox = default;
        for (handle, selected, other) in [
            (&handles()[0], &native, &provider),
            (&handles()[1], &provider, &native),
        ] {
            let before_selected = selected.calls.load(Ordering::SeqCst);
            let before_other = other.calls.load(Ordering::SeqCst);
            let input = request(&policy);
            let before = root.reserved_work_bytes();
            let operation = router.exchange_admitted(handle, input, &policy).unwrap();
            assert_eq!(
                operation.requested_bytes(),
                selected.bytes.load(Ordering::SeqCst)
            );
            assert_eq!(
                root.reserved_work_bytes() - before,
                operation.requested_bytes()
            );
            let output = operation.now_or_never().unwrap().unwrap();
            assert!(output.belongs_to(&policy));
            assert_eq!(selected.calls.load(Ordering::SeqCst), before_selected + 1);
            assert_eq!(other.calls.load(Ordering::SeqCst), before_other);
            drop(output);
            root.assert_idle();
        }
    }
}

#[test]
fn b6_ingress_router_missing_provider_refuses_without_allocation_or_failover() {
    let (root, policy) = policy();
    let unused = Arc::new(Transport::default());
    for (index, router) in [
        router(None, Some(Arc::new(backend(unused.clone())))),
        router(Some(Arc::new(backend(unused.clone()))), None),
    ]
    .into_iter()
    .enumerate()
    {
        let handle = &handles()[index];
        let input = request(&policy);
        let recovery = command(&policy);
        let mut probe = AllocationProbe::new();
        let error = probe
            .track(|| router.exchange_admitted(handle, input, &policy))
            .unwrap_err();
        assert!(matches!(error, AdmittedError::Unavailable(_)));
        let error = probe
            .track(|| router.run_recovery_command_admitted(handle, recovery, &policy))
            .unwrap_err();
        assert!(matches!(error, AdmittedError::Unavailable(_)));
        assert_eq!(probe.allocated(), 0);
        assert_eq!(unused.calls.load(Ordering::SeqCst), 0);
        root.assert_idle();
    }
}

#[test]
fn b6_ingress_router_rejects_foreign_custody_before_backend_selection() {
    let (root, policy) = policy();
    let (foreign_root, foreign) = self::policy();
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let transport = Arc::new(Transport::default());
    let recovery = Arc::new(Recovery::default());
    let router = router(
        Some(recovery.clone()),
        Some(Arc::new(backend(transport.clone()))),
    );
    for owner in [&foreign, second.policy()] {
        let input = request(owner);
        let command = command(owner);
        let handles = handles();
        let mut probe = AllocationProbe::new();
        let exchange = probe
            .track(|| router.exchange_admitted(&handles[1], input, first.policy()))
            .unwrap_err();
        let recovery = probe
            .track(|| router.run_recovery_command_admitted(&handles[0], command, first.policy()))
            .unwrap_err();
        for error in [exchange, recovery] {
            assert!(matches!(
                error.refusal(),
                Some(AdmissionRefusal::InvalidPlan { .. })
            ));
        }
        assert_eq!(probe.allocated(), 0);
    }
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    assert_eq!(recovery.calls.load(Ordering::SeqCst), 0);
    drop((first, second));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[test]
fn b6_ingress_router_keeps_unsupported_and_capacity_refusals_without_fallback() {
    let (root, policy) = policy();
    let provider = Arc::new(Transport::default());
    let router = router(
        Some(Arc::new(crate::hosted::InMemoryHostedBackend::default())),
        Some(Arc::new(backend(provider.clone()))),
    );
    let handles = handles();
    assert!(matches!(
        router.exchange_admitted(&handles[0], request(&policy), &policy),
        Err(AdmittedError::Unsupported(_))
    ));
    assert!(matches!(
        router.run_recovery_command_admitted(&handles[0], command(&policy), &policy),
        Err(AdmittedError::Unsupported(_))
    ));
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    let input = request(&policy);
    let pressure = policy
        .admit(
            IngressClass::Data,
            plan(policy.budget(IngressClass::Data).available_bytes()),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| router.exchange_admitted(&handles[1], input, &policy))
        .unwrap_err();
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(provider.started.load(Ordering::SeqCst), 0);
    drop(pressure);
    root.assert_idle();
}

#[derive(Default)]
struct Recovery {
    calls: AtomicUsize,
    pending: bool,
}

fn command(policy: &IngressOwnershipPolicy) -> OwnedWork<crate::hosted::RuntimeRecoveryCommand> {
    IngressWorkScope::new(policy.admit(IngressClass::Control, plan(4096)).unwrap())
        .initialize_owned(|| crate::hosted::RuntimeRecoveryCommand {
            program: "/fixture".into(),
            args: vec![],
            input: vec![17; 1024],
        })
}

#[async_trait::async_trait]
impl HostedBackend for Recovery {
    fn run_recovery_command_admitted<'a>(
        &'a self,
        _: &'a RuntimeHandle,
        command: OwnedWork<crate::hosted::RuntimeRecoveryCommand>,
        policy: &IngressOwnershipPolicy,
    ) -> std::result::Result<AdmittedOperation<'a, Vec<u8>>, AdmittedError> {
        assert!(command.belongs_to(policy));
        self.calls.fetch_add(1, Ordering::SeqCst);
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Control,
            plan(0),
            move |_| async move {
                if self.pending {
                    std::future::pending::<()>().await;
                }
                Ok(command.map(|command| command.input))
            },
        )
    }
    async fn run_recovery_command(
        &self,
        _: &RuntimeHandle,
        command: &crate::hosted::RuntimeRecoveryCommand,
    ) -> Result<Vec<u8>> {
        Ok(command.input.clone())
    }
    async fn provision(&self, _: ProviderSandboxRequest) -> Result<RuntimeHandle> {
        panic!("no allocation")
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        panic!("no legacy exchange")
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: HostedAgentOpenSettings,
        _: mpsc::Sender<ConversationUpdate>,
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("no agent open")
    }
    async fn teardown(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("no teardown")
    }
}

#[test]
fn b6_ingress_router_recovery_keeps_output_ownership_and_legacy_route() {
    let (root, policy) = policy();
    let selected = Arc::new(Recovery::default());
    let other = Arc::new(Recovery::default());
    let mut router = router(Some(selected.clone()), Some(other.clone()));
    let handle = &handles()[0];
    for default in [false, true] {
        router.allocate_agent_sandbox = default;
        let input = command(&policy);
        let address = input.input.as_ptr();
        let legacy = router
            .run_recovery_command(handle, &input)
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(legacy, input.input);
        let before = root.reserved_work_bytes();
        let operation = router
            .run_recovery_command_admitted(handle, input, &policy)
            .unwrap();
        assert_eq!(
            root.reserved_work_bytes() - before,
            operation.requested_bytes()
        );
        let output = operation.now_or_never().unwrap().unwrap();
        assert_eq!(output.as_ptr(), address);
        assert_eq!(&**output, legacy.as_slice());
        assert!(output.belongs_to(&policy));
        drop(output);
        root.assert_idle();
    }
    assert_eq!(selected.calls.load(Ordering::SeqCst), 2);
    assert_eq!(other.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn b6_ingress_router_cancellation_retains_recovery_attachment_until_drop() {
    let (root, policy) = policy();
    let attachment = policy.admit_attachment().unwrap();
    let blocked_slot = policy.admit_attachment().unwrap();
    let router = router(
        Some(Arc::new(Recovery {
            pending: true,
            ..Default::default()
        })),
        None,
    );
    let handles = handles();
    let mut operation = router
        .run_recovery_command_admitted(
            &handles[0],
            command(attachment.policy()),
            attachment.policy(),
        )
        .unwrap();
    assert!((&mut operation).now_or_never().is_none());
    drop(attachment);
    assert!(policy.admit_attachment().is_err());
    drop(operation);
    drop(policy.admit_attachment().unwrap());
    drop(blocked_slot);
    root.assert_idle();
}

#[test]
fn b6_ingress_router_forwarding_allocates_only_the_selected_operation() {
    let (root, policy) = policy();
    let transport = Arc::new(Transport::default());
    let leaf = Arc::new(backend(transport));
    let router = router(None, Some(leaf.clone()));
    let handle = RuntimeHandle::new("provider");
    let mut totals = Vec::new();
    for forwarded in [false, true] {
        let source = request(&policy);
        let mut probe = AllocationProbe::new();
        let operation = probe.track(|| {
            if forwarded {
                router.exchange_admitted(&handle, source, &policy)
            } else {
                leaf.exchange_admitted(&handle, source, &policy)
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

#[test]
fn b6_ingress_router_result_remains_charged_through_physical_deallocation() {
    use crate::allocation_probe::deallocation;
    const TEST: &str = "hosted::agent_sandbox_backend::routing::ingress_tests::b6_ingress_router_result_remains_charged_through_physical_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = policy();
    let attachment = policy.admit_attachment().unwrap();
    let blocked_slot = policy.admit_attachment().unwrap();
    let router = router(Some(Arc::new(Recovery::default())), None);
    let handle = &handles()[0];
    let mut probe = AllocationProbe::new();
    let (source, pause) = probe.pause_deallocation_containing(
        1024,
        || command(attachment.policy()),
        |source| source.input.as_ptr() as usize,
    );
    let output = router
        .run_recovery_command_admitted(handle, source, attachment.policy())
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    drop(attachment);
    let charged = root.reserved_work_bytes();
    pause.inspect_while_dropping(
        move || drop(output),
        || {
            assert_eq!(root.reserved_work_bytes(), charged);
            assert!(policy.admit_attachment().is_err());
        },
    );
    assert_eq!(probe.live(), 0);
    drop(policy.admit_attachment().unwrap());
    drop(blocked_slot);
    root.assert_idle();
}
