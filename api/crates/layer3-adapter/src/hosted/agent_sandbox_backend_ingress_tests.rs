use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::hosted::provider_backend::ingress_tests::{plan, policy};
use crate::ingress_budget::{IngressClass, IngressWorkScope};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

#[derive(Default)]
struct Transport {
    calls: AtomicUsize,
    polled: AtomicUsize,
    legacy: AtomicUsize,
    pending: bool,
    error: Option<AdmittedError>,
}

#[async_trait::async_trait]
impl HostedControlTransport for Transport {
    fn exchange_admitted<'a>(
        &'a self,
        _: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> std::result::Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        self.calls.fetch_add(1, SeqCst);
        assert!(request.belongs_to(policy));
        AdmittedOperation::from_owned(policy, IngressClass::Data, plan(0), move |_| async move {
            self.polled.fetch_add(1, SeqCst);
            if self.pending {
                std::future::pending::<()>().await;
            }
            if let Some(error) = &self.error {
                return Err(error.clone());
            }
            Ok(request.map(|request| {
                let RequestFrame::CacheSkillArchive(archive) = request else {
                    panic!("unexpected request reached native transport")
                };
                ResponseFrame::CacheSkillArchive {
                    sha256: archive.sha256,
                }
            }))
        })
    }

    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> Result<ResponseFrame> {
        self.legacy.fetch_add(1, SeqCst);
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
    ) -> Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        panic!("exchange cannot open an Agent")
    }
    async fn retire(&self, _: &RuntimeHandle) -> Result<()> {
        panic!("exchange cannot retire a runtime")
    }
}

fn backend(transport: Arc<dyn HostedControlTransport>) -> (AgentSandboxBackend, Arc<AtomicUsize>) {
    let reads = Arc::new(AtomicUsize::new(0));
    let observed = reads.clone();
    let service = tower::service_fn(move |_: http::Request<kube::client::Body>| {
        observed.fetch_add(1, SeqCst);
        std::future::ready(Ok::<_, std::convert::Infallible>(
            http::Response::builder().status(404)
                .body(kube::client::Body::from(
                    br#"{"status":"Failure","reason":"NotFound","message":"synthetic absent runtime","code":404}"#.to_vec(),
                )).unwrap(),
        ))
    });
    let client = AgentSandboxClient::new(
        kube::Client::new(service, "synthetic"),
        crate::agent_sandbox::AgentSandboxConfig {
            namespace: "synthetic".into(),
            deployment: "synthetic".into(),
            runtime_class: None,
            service_account: "synthetic".into(),
            image_pull_secrets: vec![],
            template_network_policy: None,
            template_fs_group_change_policy: None,
            native_claim_guard_enabled: false,
            runtime_startup_breaker_enabled: false,
            control_key: "fixture".repeat(8),
        },
        "synthetic",
    )
    .unwrap();
    (AgentSandboxBackend::new(client, transport), reads)
}

fn request(policy: &IngressOwnershipPolicy) -> OwnedWork<RequestFrame> {
    use sha2::{Digest, Sha256};
    IngressWorkScope::new(policy.admit(IngressClass::Data, plan(4096)).unwrap()).initialize_owned(
        || {
            RequestFrame::CacheSkillArchive(crate::SkillArchive {
                sha256: format!("{:x}", Sha256::digest(b"synthetic")),
                bytes: b"synthetic".to_vec(),
            })
        },
    )
}

fn handle() -> RuntimeHandle {
    RuntimeHandle::new("agent-sandbox:synthetic:uid")
}
fn address(request: &RequestFrame) -> usize {
    let RequestFrame::CacheSkillArchive(archive) = request else {
        panic!("archive fixture")
    };
    archive.sha256.as_ptr() as usize
}

#[tokio::test]
async fn b6_ingress_native_rejects_foreign_root_attachment_and_unowned_requests() {
    let (root, policy) = policy();
    let (foreign_root, foreign) = self::policy();
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let transport = Arc::new(Transport::default());
    let (backend, reads) = backend(transport.clone());
    let handle = handle();
    for input in [
        request(&foreign),
        request(second.policy()),
        IngressWorkScope::inherit_owned(async { RequestFrame::Resources }).await,
    ] {
        let mut probe = AllocationProbe::new();
        let error = probe
            .track(|| backend.exchange_admitted(&handle, input, first.policy()))
            .unwrap_err();
        assert!(matches!(
            error.refusal(),
            Some(AdmissionRefusal::InvalidPlan { .. })
        ));
        assert_eq!(probe.allocated(), 0);
    }
    assert_eq!(transport.calls.load(SeqCst), 0);
    assert_eq!(transport.legacy.load(SeqCst), 0);
    assert_eq!(reads.load(SeqCst), 0);
    drop((first, second));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[tokio::test]
async fn b6_ingress_native_status_refuses_before_wire_or_native_reads_under_pressure() {
    let (root, policy) = policy();
    let transport = Arc::new(Transport::default());
    let (backend, reads) = backend(transport.clone());
    let handle = handle();
    let input = crate::hosted::provider_backend::ingress_tests::request(&policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| backend.exchange_admitted(&handle, input, &policy))
        .unwrap_err();
    assert!(matches!(
        error,
        AdmittedError::Unsupported("native status validation requires admitted reads")
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(transport.calls.load(SeqCst), 0);
    assert_eq!(transport.legacy.load(SeqCst), 0);
    assert_eq!(reads.load(SeqCst), 0);
    drop(pressure);
    root.assert_idle();
}

#[tokio::test]
async fn b6_ingress_native_retains_the_same_success_payload_and_attachment() {
    let (root, policy) = policy();
    let attachment = policy.admit_attachment().unwrap();
    let blocked = policy.admit_attachment().unwrap();
    let input = request(attachment.policy());
    let pointer = address(&input);
    let transport = Arc::new(Transport::default());
    let (backend, reads) = backend(transport.clone());
    let handle = handle();
    let response = backend
        .exchange_admitted(&handle, input, attachment.policy())
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    assert!(response.belongs_to(attachment.policy()));
    let ResponseFrame::CacheSkillArchive { sha256 } = &*response else {
        panic!("archive response")
    };
    assert_eq!(sha256.as_ptr() as usize, pointer);
    assert_eq!(transport.calls.load(SeqCst), 1);
    assert_eq!(transport.polled.load(SeqCst), 1);
    assert_eq!(transport.legacy.load(SeqCst), 0);
    assert_eq!(reads.load(SeqCst), 0);
    drop((attachment, backend, transport));
    let shared = response.into_shared_admitted().unwrap();
    let retained = shared.clone();
    drop(shared);
    assert!(policy.admit_attachment().is_err());
    drop(retained);
    drop(policy.admit_attachment().unwrap());
    drop(blocked);
    root.assert_idle();
}

#[tokio::test]
async fn b6_ingress_native_preserves_error_identity_and_custody() {
    let (root, policy) = policy();
    let scope = IngressWorkScope::new(policy.admit(IngressClass::Data, plan(4096)).unwrap());
    let error =
        AdmittedError::Adapter(scope.initialize(|| AdapterError::Rejected("synthetic".into())));
    let pointer = error.as_adapter().unwrap() as *const AdapterError;
    let transport = Arc::new(Transport {
        error: Some(error.clone()),
        ..Default::default()
    });
    let (backend, _) = backend(transport.clone());
    let handle = handle();
    let returned = backend
        .exchange_admitted(&handle, request(&policy), &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        returned.as_adapter().unwrap() as *const AdapterError,
        pointer
    );
    drop((error, scope, transport, backend));
    assert_eq!(root.reserved_work_bytes(), 4096);
    drop(returned);
    root.assert_idle();
}

#[tokio::test]
async fn b6_ingress_native_capacity_refusal_precedes_child_future_and_poll() {
    let (root, policy) = policy();
    let transport = Arc::new(Transport::default());
    let (backend, reads) = backend(transport.clone());
    let handle = handle();
    let input = request(&policy);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![0u8; available - 1024]);
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| backend.exchange_admitted(&handle, input, &policy))
        .unwrap_err();
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(transport.calls.load(SeqCst), 1);
    assert_eq!(transport.polled.load(SeqCst), 0);
    assert_eq!(transport.legacy.load(SeqCst), 0);
    assert_eq!(reads.load(SeqCst), 0);
    drop(pressure);
    root.assert_idle();
}

#[tokio::test]
async fn b6_ingress_native_cancellation_retains_input_until_operation_drop() {
    let (root, policy) = policy();
    for polled in [false, true] {
        let first = policy.admit_attachment().unwrap();
        let blocked = policy.admit_attachment().unwrap();
        let transport = Arc::new(Transport {
            pending: true,
            ..Default::default()
        });
        let (backend, reads) = backend(transport.clone());
        let handle = handle();
        let mut operation = backend
            .exchange_admitted(&handle, request(first.policy()), first.policy())
            .unwrap();
        if polled {
            assert!((&mut operation).now_or_never().is_none());
        }
        assert_eq!(transport.polled.load(SeqCst), usize::from(polled));
        drop(first);
        assert!(policy.admit_attachment().is_err());
        assert!(root.reserved_work_bytes() >= 4096);
        drop(operation);
        drop(policy.admit_attachment().unwrap());
        assert_eq!(reads.load(SeqCst), 0);
        drop(blocked);
        root.assert_idle();
    }
}

#[tokio::test]
async fn b6_ingress_native_forwarding_adds_no_allocation_to_the_child_operation() {
    let (root, policy) = policy();
    let transport = Arc::new(Transport::default());
    let (backend, _) = backend(transport.clone());
    let backend = Arc::new(backend);
    let router = RuntimeBackends {
        agent_sandbox: Some(backend.clone()),
        opensandbox: None,
        allocate_agent_sandbox: false,
        routing: Arc::new(Routes),
        home_routes: None,
    };
    let handle = handle();
    let mut totals = [0; 3];
    for (forwarded, total) in totals.iter_mut().enumerate() {
        let input = request(&policy);
        let before = root.reserved_work_bytes();
        let mut probe = AllocationProbe::new();
        let operation = probe.track(|| {
            match forwarded {
                0 => transport.exchange_admitted(&handle, input, &policy),
                1 => backend.exchange_admitted(&handle, input, &policy),
                _ => router.exchange_admitted(&handle, input, &policy),
            }
            .unwrap()
        });
        assert_eq!(
            root.reserved_work_bytes() - before,
            operation.requested_bytes()
        );
        assert!(probe.peak() <= operation.requested_bytes());
        *total = probe.allocated();
        drop(operation);
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }
    assert!(totals[0] > 0);
    assert_eq!(totals[0], totals[1]);
    assert_eq!(totals[0], totals[2]);
    assert_eq!(transport.calls.load(SeqCst), 3);
    assert_eq!(transport.legacy.load(SeqCst), 0);
}

#[tokio::test]
async fn b6_ingress_native_legacy_exchange_keeps_its_status_validation_path() {
    let transport = Arc::new(Transport::default());
    let (backend, reads) = backend(transport.clone());
    let handle = handle();
    assert_eq!(
        backend
            .exchange(&handle, RequestFrame::Resources)
            .await
            .unwrap(),
        ResponseFrame::CacheSkillArchive {
            sha256: "legacy".into()
        }
    );
    let error = backend
        .exchange(
            &handle,
            RequestFrame::RuntimeStatus(RuntimeStatusRequest {
                sandbox_id: "synthetic".into(),
                generation: 1,
                home_identity: "synthetic".into(),
            }),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, AdapterError::Unavailable(ref detail) if detail == "status runtime is absent")
    );
    assert_eq!(reads.load(SeqCst), 1);
    assert_eq!(transport.legacy.load(SeqCst), 1);
    assert_eq!(transport.calls.load(SeqCst), 0);
}

#[tokio::test]
async fn b6_ingress_native_result_remains_charged_until_physical_free() {
    const TEST: &str = "hosted::agent_sandbox_backend::ingress_tests::b6_ingress_native_result_remains_charged_until_physical_free";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = policy();
    let first = policy.admit_attachment().unwrap();
    let blocked = policy.admit_attachment().unwrap();
    let (backend, _) = backend(Arc::new(Transport::default()));
    let handle = handle();
    let mut probe = AllocationProbe::new();
    let (input, pause) =
        probe.pause_deallocation_containing(64, || request(first.policy()), |input| address(input));
    let response = backend
        .exchange_admitted(&handle, input, first.policy())
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    drop(first);
    let charged = root.reserved_work_bytes();
    pause.inspect_while_dropping(
        move || drop(response),
        || {
            assert_eq!(root.reserved_work_bytes(), charged);
            assert!(policy.admit_attachment().is_err());
        },
    );
    assert_eq!(probe.live(), 0);
    drop(policy.admit_attachment().unwrap());
    drop(blocked);
    root.assert_idle();
}

struct Routes;
#[async_trait::async_trait]
impl RuntimeRoutingStore for Routes {
    async fn select(&self, _: &str, _: u64, _: bool) -> Result<bool> {
        panic!("existing native exchange cannot select a creation route")
    }
}

#[tokio::test]
async fn b6_ingress_native_unsupported_transport_does_not_fall_back_to_legacy() {
    let (root, policy) = policy();
    let backend = super::history_tests::empty_native();
    let handle = handle();
    let input = request(&policy);
    let mut probe = AllocationProbe::new();
    let error = probe
        .track(|| backend.exchange_admitted(&handle, input, &policy))
        .unwrap_err();
    assert!(matches!(error, AdmittedError::Unsupported(_)));
    assert_eq!(probe.allocated(), 0);
    root.assert_idle();
}
