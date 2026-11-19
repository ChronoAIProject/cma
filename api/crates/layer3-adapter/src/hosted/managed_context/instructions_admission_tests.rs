use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{ClassBounds, IngressBudget};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, WireFailure, WireOperation};
use crate::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

pub(in crate::hosted) struct Source {
    status: Arc<super::super::receipt_admission::tests::Source>,
    response: ResponseFrame,
    fault: u8,
    foreign: Option<IngressOwnershipPolicy>,
    pub(in crate::hosted) reads: AtomicUsize,
}

#[async_trait::async_trait]
impl HostedBackend for Source {
    fn exchange_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        if matches!(&*request, RequestFrame::RuntimeStatus(_)) {
            return self.status.exchange_admitted(handle, request, policy);
        }
        let RequestFrame::ReadAgentFile(read) = &*request else {
            panic!("unexpected request")
        };
        assert!(request.belongs_to(policy));
        assert!(read.owner_access);
        assert!(read.protected_paths.is_empty());
        assert_eq!(read.sandbox_id, "synthetic-sandbox");
        assert_eq!(read.runtime_generation, 3);
        assert_eq!(read.boot_id.as_bytes(), &[b'a'; 32]);
        assert_eq!(read.max_bytes, MAX_INSTRUCTIONS as u64);
        let fields = match &self.response {
            ResponseFrame::ReadAgentFile(file) => [
                file.path.len(),
                file.sha256.len(),
                file.content_base64.len(),
            ],
            ResponseFrame::Failure(failure) => [failure.code.len(), failure.detail.len(), 0],
            _ => unreachable!(),
        };
        let plan = AllocationPlan::checked(
            "synthetic instruction reply",
            &[fields[0], fields[1], fields[2], 2048],
        )?;
        AdmittedOperation::new(
            self.foreign.as_ref().unwrap_or(policy),
            IngressClass::Data,
            plan,
            move |_| async move {
                self.reads.fetch_add(1, SeqCst);
                if self.fault == 10 {
                    std::future::pending::<()>().await;
                }
                if self.fault == 7 {
                    return Err(AdapterError::Unavailable("x".repeat(2048)));
                }
                let result = self.response.clone();
                drop(request);
                Ok(result)
            },
        )
    }
    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        panic!("instruction admission cannot invoke a legacy producer")
    }
    async fn provision(
        &self,
        _: crate::provider::ProviderSandboxRequest,
    ) -> crate::Result<RuntimeHandle> {
        unreachable!()
    }
    async fn teardown(&self, _: &RuntimeHandle) -> crate::Result<()> {
        unreachable!()
    }
    async fn inventory(&self) -> crate::Result<Vec<RuntimeObservation>> {
        unreachable!()
    }
    async fn open_agent(
        &self,
        _: &RuntimeHandle,
        _: FencedRequest<AgentLaunch>,
        _: Option<String>,
        _: crate::hosted::HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!()
    }
}

pub(in crate::hosted) fn binding(
    fault: u8,
    content: &str,
    foreign: Option<IngressOwnershipPolicy>,
) -> (HostedContextRefresh, Arc<Source>) {
    let (base, status) = super::super::receipt_admission::tests::binding(
        if fault == 12 {
            1
        } else if fault == 13 {
            2
        } else {
            0
        },
        None,
    );
    let content = if fault == 6 {
        "different instructions"
    } else {
        content
    };
    let mut file = crate::AgentFileReadReceipt {
        path: base.0.path.clone(),
        size_bytes: content.len() as u64,
        sha256: format!("{:x}", Sha256::digest(content.as_bytes())),
        content_base64: base64::engine::general_purpose::STANDARD.encode(content),
    };
    match fault {
        1 => file.path.push_str(".other"),
        2 => file.content_base64 = "invalid!".into(),
        3 => file.sha256 = "b".repeat(64),
        4 => file.size_bytes += 1,
        5 => file.size_bytes = MAX_INSTRUCTIONS as u64 + 1,
        _ => {}
    }
    let response = if matches!(fault, 8 | 14) {
        ResponseFrame::Failure(WireFailure {
            operation: WireOperation::ReadAgentFile,
            code: "synthetic_failure".into(),
            detail: if fault == 14 {
                content.into()
            } else {
                "retained detail".into()
            },
            retryable: true,
        })
    } else if fault == 11 {
        ResponseFrame::Failure(WireFailure {
            operation: WireOperation::WriteAgentFile,
            code: "wrong_operation".into(),
            detail: "retained detail".into(),
            retryable: false,
        })
    } else {
        ResponseFrame::ReadAgentFile(file)
    };
    let source = Arc::new(Source {
        status,
        response,
        fault,
        foreign,
        reads: AtomicUsize::new(0),
    });
    let context = HostedContextRefresh::new(
        source.clone(),
        base.0.handle.clone(),
        base.0.identity.clone(),
        "/home/cma/.cma/agents/synthetic/home",
        base.0.cwd.clone(),
        Vec::new(),
    )
    .unwrap();
    (context, source)
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(2 * 1024 * 1024);
    let floor = AllocationPlan::checked("instructions fixture floor", &[512 * 1024]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(
            policy
                .admit(
                    class,
                    AllocationPlan::checked("shared pool backing", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }
    (root, policy)
}

#[test]
fn instruction_confirmation_preserves_validation_and_owned_failures() {
    for fault in [0, 1, 2, 3, 4, 5, 6, 7, 8, 11, 12, 13] {
        let (root, policy) = policy();
        let (binding, source) = binding(fault, "exact instructions", None);
        let result = binding
            .confirm_instructions_admitted("exact instructions", &policy)
            .unwrap()
            .now_or_never()
            .unwrap();
        assert_eq!(result.is_ok(), fault == 0, "fault={fault}: {result:?}");
        assert_eq!(source.reads.load(SeqCst), usize::from(fault < 12));
        if let Err(error) = &result {
            assert!(error.belongs_to(&policy));
            let expected = match fault {
                1 | 6 => instruction_mismatch(),
                2 => AdapterError::Rejected("runtime returned invalid file encoding".into()),
                3 | 4 => AdapterError::Rejected("runtime returned an invalid file digest".into()),
                5 => AdapterError::Rejected("file response exceeds its size limit".into()),
                7 => AdapterError::Unavailable("x".repeat(2048)),
                8 => AdapterError::Unavailable("synthetic_failure: retained detail".into()),
                11 => AdapterError::unexpected_reply("managed context"),
                12 => AdapterError::Rejected(
                    "runtime status does not prove the requested identity".into(),
                ),
                13 => AdapterError::Unsupported(
                    "runtime is not ready for managed context refresh".into(),
                ),
                _ => unreachable!(),
            };
            assert_eq!(error.as_adapter(), Some(&expected));
        }
        drop((binding, source));
        assert!(root.available_bytes() < root.capacity_bytes());
        drop(result);
        assert_eq!(root.available_bytes(), root.capacity_bytes());
    }
}

#[test]
fn instruction_source_refuses_legacy_and_foreign_producers_before_poll() {
    let (root, policy) = policy();
    let (legacy, files) = super::super::writer_tests::binding(0);
    let error = legacy
        .confirm_instructions_admitted("instructions", &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, AdmittedError::Unsupported(_)));
    assert_eq!(files.status_reads.load(SeqCst), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    let (foreign_root, foreign) = self::policy();
    let (binding, source) = binding(0, "instructions", Some(foreign));
    let error = binding
        .confirm_instructions_admitted("instructions", &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(source.reads.load(SeqCst), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    assert_eq!(
        foreign_root.available_bytes(),
        foreign_root.capacity_bytes()
    );
}

#[test]
fn instruction_cancellation_and_pressure_release_every_owner() {
    let (root, policy) = policy();
    let (binding, source) = binding(10, "instructions", None);
    let mut pending = Box::pin(
        binding
            .confirm_instructions_admitted("instructions", &policy)
            .unwrap(),
    );
    assert!(pending.as_mut().now_or_never().is_none());
    assert_eq!(source.reads.load(SeqCst), 1);
    drop(pending);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "instruction pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        binding
            .confirm_instructions_admitted("instructions", &policy)
            .unwrap_err()
    });
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.reads.load(SeqCst), 1);
    drop((probe, pressure));
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn instruction_source_error_stays_charged_through_last_clone() {
    if crate::allocation_probe::deallocation::isolated("hosted::managed_context::instructions_admission::tests::instruction_source_error_stays_charged_through_last_clone") { return; }
    let (root, policy) = policy();
    let (binding, source) = binding(7, "instructions", None);
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        binding
            .confirm_instructions_admitted("instructions", &policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err()
    });
    let clone = probe.track(|| error.clone());
    drop((binding, source, error));
    assert!(probe.live() >= 2048);
    assert!(probe.live() <= root.capacity_bytes() - root.available_bytes());
    drop(clone);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn instruction_decode_admits_peak_overlap_and_refuses_before_decoding() {
    if crate::allocation_probe::deallocation::isolated("hosted::managed_context::instructions_admission::tests::instruction_decode_admits_peak_overlap_and_refuses_before_decoding") { return; }
    let (root, policy) = policy();
    let expected = "i".repeat(MAX_INSTRUCTIONS);
    let (_, source) = binding(0, &expected, None);
    let ResponseFrame::ReadAgentFile(file) = &source.response else {
        unreachable!()
    };
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        verify_instructions(file, &expected, &policy)
            .now_or_never()
            .unwrap()
            .unwrap()
    });
    let charged = root.capacity_bytes() - root.available_bytes();
    assert!(probe.peak() >= MAX_INSTRUCTIONS);
    assert!(
        probe.peak() <= charged,
        "decode peak={} charged={charged}",
        probe.peak()
    );
    eprintln!(
        "instruction_decode peak={} retained={} charged={charged}",
        probe.peak(),
        probe.live()
    );
    drop(result);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
    drop(probe);
    let pressure = policy
        .admit(
            IngressClass::Transformation,
            AllocationPlan::checked(
                "decode pressure",
                &[policy
                    .budget(IngressClass::Transformation)
                    .available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        verify_instructions(file, &expected, &policy)
            .now_or_never()
            .unwrap()
            .unwrap_err()
    });
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    drop(probe);
    let mut probe = AllocationProbe::new();
    let baseline = probe.track(|| file.content().unwrap());
    assert!(probe.peak() >= MAX_INSTRUCTIONS);
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        0
    );
    eprintln!("legacy_instruction_decode peak={} charged=0", probe.peak());
    drop((baseline, pressure));
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}

#[test]
fn instruction_wire_error_accounts_formatting_overlap_and_retained_clones() {
    if crate::allocation_probe::deallocation::isolated("hosted::managed_context::instructions_admission::tests::instruction_wire_error_accounts_formatting_overlap_and_retained_clones") { return; }
    let (root, policy) = policy();
    let detail = "e".repeat(64 * 1024);
    let (binding, source) = binding(14, &detail, None);
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        binding
            .confirm_instructions_admitted("instructions", &policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err()
    });
    let charged = root.capacity_bytes() - root.available_bytes();
    assert!(probe.peak() >= detail.len() * 2);
    assert!(
        probe.peak() <= charged,
        "wire error peak={} charged={charged}",
        probe.peak()
    );
    assert_eq!(
        error.as_adapter(),
        Some(&AdapterError::Unavailable(format!(
            "synthetic_failure: {detail}"
        )))
    );
    let clone = probe.track(|| error.clone());
    drop((binding, source, error));
    assert!(probe.live() >= detail.len());
    assert!(probe.live() <= clone.requested_bytes());
    eprintln!(
        "instruction_wire_error peak={} retained={} charged={charged}",
        probe.peak(),
        probe.live()
    );
    drop(clone);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), root.capacity_bytes());
}
