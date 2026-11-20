use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::hosted::managed_context::receipt_admission::tests as status_fixture;
use crate::hosted::{HostedAgentOpenSettings, HostedBackend};
use crate::ingress_budget::{AdmittedConversationSettingsSupport, OwnedWork};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame};
use crate::{
    AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeHandle, RuntimeObservation,
};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

struct Source {
    status: crate::ingress_budget::SharedOwned<status_fixture::Source>,
    calls: AtomicUsize,
    fault: u8,
    output: Vec<u8>,
    expected_input: Vec<u8>,
    foreign: Option<IngressOwnershipPolicy>,
}

#[async_trait::async_trait]
impl HostedBackend for Source {
    fn exchange_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        request: OwnedWork<RequestFrame>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, ResponseFrame>, AdmittedError> {
        let status = self.status.exchange_admitted(handle, request, policy)?;
        AdmittedOperation::from_owned(
            policy,
            IngressClass::Data,
            AllocationPlan::checked("synthetic status forwarding", &[0])?,
            move |_| async move {
                let response = status.await?;
                Ok(response.map(|mut response| {
                    if self.fault == 1 && self.status.calls.load(SeqCst) > 1 {
                        let ResponseFrame::RuntimeStatus(status) = &mut response else {
                            panic!("status expected")
                        };
                        status.boot_id.replace_range(..1, "b");
                    }
                    response
                }))
            },
        )
    }

    fn run_recovery_command_admitted<'a>(
        &'a self,
        handle: &'a RuntimeHandle,
        command: OwnedWork<RuntimeRecoveryCommand>,
        policy: &IngressOwnershipPolicy,
    ) -> Result<AdmittedOperation<'a, Vec<u8>>, AdmittedError> {
        assert!(command.belongs_to(policy));
        if self.fault == 4 {
            return Err(AdmittedError::Unsupported("synthetic unsupported recovery"));
        }
        AdmittedOperation::new(
            self.foreign.as_ref().unwrap_or(policy),
            IngressClass::Data,
            AllocationPlan::checked(
                "synthetic recovery output",
                &[self.output.len(), "synthetic recovery failure".len()],
            )?,
            move |_| async move {
                self.calls.fetch_add(1, SeqCst);
                assert_eq!(handle.as_str(), "synthetic-runtime");
                assert_eq!(command.program, "/usr/bin/python3");
                assert_eq!(command.args, ["-I", "-c", SCRIPT]);
                assert_eq!(command.input, self.expected_input);
                if self.fault == 2 {
                    return Err(crate::AdapterError::Unavailable(
                        "synthetic recovery failure".into(),
                    ));
                }
                if self.fault == 3 {
                    std::future::pending::<()>().await;
                }
                Ok(self.output.clone())
            },
        )
    }

    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        panic!("admitted goal inspection cannot invoke legacy status")
    }
    async fn run_recovery_command(
        &self,
        _: &RuntimeHandle,
        _: &RuntimeRecoveryCommand,
    ) -> crate::Result<Vec<u8>> {
        panic!("admitted goal inspection cannot invoke legacy recovery")
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
        _: HostedAgentOpenSettings,
        _: tokio::sync::mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        unreachable!()
    }
}

fn binding(
    policy: &IngressOwnershipPolicy,
    fault: u8,
    session: &str,
    output: Vec<u8>,
    foreign: Option<IngressOwnershipPolicy>,
) -> (
    HostedContextRefresh,
    crate::ingress_budget::SharedOwned<Source>,
) {
    let (base, status) = status_fixture::binding(
        policy,
        if fault == 5 {
            1
        } else if fault == 6 {
            2
        } else {
            0
        },
        None,
    );
    let source = super::super::binding_fixture::source(
        policy,
        output.len() + session.len() * 6 + 1024,
        || Source {
            status,
            calls: AtomicUsize::new(0),
            fault,
            output: output.clone(),
            expected_input: serde_json::to_vec(&serde_json::json!({
                "home":"/home/cma/.cma/agents/synthetic/home", "session": session,
            }))
            .unwrap(),
            foreign,
        },
    );
    let mut sources = super::super::binding_fixture::sources(&base);
    sources.backend = source.clone().into();
    let binding = super::super::binding_fixture::rebuild(policy, &base, sources, &[]);
    (binding, source)
}

fn output(session: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({"version":1,"session":session,"goal_absent":true}))
        .unwrap()
}

fn policy() -> (crate::ingress_budget::IngressBudget, IngressOwnershipPolicy) {
    let fixture = crate::hosted::acp::ingress::fixture::fixture();
    let root = fixture.root.clone();
    let policy = fixture.policy.clone();
    drop(fixture);
    // Initialize shared semaphore backing before operation measurements.
    for class in [
        IngressClass::Data,
        IngressClass::Transformation,
        IngressClass::Control,
    ] {
        drop(
            policy
                .admit(
                    class,
                    AllocationPlan::checked("policy backing", &[1]).unwrap(),
                )
                .unwrap(),
        );
    }
    (root, policy)
}

#[test]
fn goal_probe_preserves_exact_home_session_and_before_after_status() {
    let (root, policy) = policy();
    for (fault, raw, expected, reads) in [
        (0, output("conversation"), true, 2),
        (1, output("conversation"), false, 2),
        (0, output("other"), false, 1),
        (
            0,
            br#"{"version":1,"session":"conversation","goal_absent":false}"#.to_vec(),
            false,
            1,
        ),
        (
            0,
            br#"{"version":1,"session":"conversation","goal_absent":true,"extra":0}"#.to_vec(),
            false,
            1,
        ),
        (
            0,
            br#"{"version":1.0,"session":"conversation","goal_absent":true}"#.to_vec(),
            false,
            1,
        ),
        (
            0,
            br#"{"version":1,"session":"convers\u0061tion","goal_absent":true}"#.to_vec(),
            true,
            2,
        ),
    ] {
        let (binding, source) = binding(&policy, fault, "conversation", raw, None);
        let result = binding
            .transport_goal_absent_admitted("conversation", &policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap();
        assert_eq!(*result, expected);
        assert_eq!(source.calls.load(SeqCst), 1);
        assert_eq!(source.status.calls.load(SeqCst), reads);
        assert!(result.belongs_to(&policy));
        drop((result, binding, source));
        root.assert_idle();
    }
}

#[test]
fn goal_errors_cancellation_and_foreign_recovery_preserve_custody() {
    let (root, policy) = policy();
    for fault in [2, 4] {
        let (binding, source) =
            binding(&policy, fault, "conversation", output("conversation"), None);
        let error = binding
            .transport_goal_absent_admitted("conversation", &policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap_err();
        assert!(error.belongs_to(&policy));
        assert_eq!(source.calls.load(SeqCst), usize::from(fault == 2));
        if fault == 2 {
            assert!(
                matches!(error.as_adapter(), Some(crate::AdapterError::Unavailable(text)) if text == "synthetic recovery failure")
            );
            assert!(root.available_bytes() < root.capacity_bytes());
        } else {
            assert!(matches!(error, AdmittedError::Unsupported(_)));
        }
        drop((error, binding, source));
        root.assert_idle();
    }
    let (binding, source) = binding(&policy, 3, "conversation", output("conversation"), None);
    let mut operation = Box::pin(
        binding
            .transport_goal_absent_admitted("conversation", &policy)
            .unwrap(),
    );
    assert!(operation.as_mut().now_or_never().is_none());
    assert_eq!(source.calls.load(SeqCst), 1);
    drop(operation);
    drop((binding, source));
    root.assert_idle();

    let (_, foreign) = self::policy();
    let (binding, source) = self::binding(
        &policy,
        0,
        "conversation",
        output("conversation"),
        Some(foreign),
    );
    let error = binding
        .transport_goal_absent_admitted("conversation", &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(error.refusal().is_some());
    assert_eq!(source.calls.load(SeqCst), 0);
    drop((binding, source));
    root.assert_idle();
}

#[test]
fn isolated_goal_admission_precedes_command_copies_and_owns_parsed_results() {
    if deallocation::isolated("hosted::managed_context::transport_goal_admission::tests::isolated_goal_admission_precedes_command_copies_and_owns_parsed_results") { return; }
    let (root, policy) = policy();
    let session = "s\"\n".repeat(300);
    let (binding, source) = binding(&policy, 0, &session, output(&session), None);
    let baseline = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        binding
            .transport_goal_absent_admitted(&session, &policy)
            .unwrap()
            .now_or_never()
            .unwrap()
            .unwrap()
    });
    assert!(*result);
    assert!(probe.peak() >= SCRIPT.len());
    assert!(probe.live() <= baseline - root.available_bytes());
    drop(result);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), baseline);
    drop(probe);
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "goal pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let (legacy, files) = crate::hosted::managed_context::writer_tests::binding(0);
    let mut probe = AllocationProbe::new();
    probe.track(|| {
        legacy
            .transport_goal_absent("retained-session")
            .now_or_never()
            .unwrap()
            .unwrap()
    });
    assert!(probe.peak() >= SCRIPT.len());
    assert_eq!(files.goal_probes.load(SeqCst), 1);
    // The legacy source retains its lazily allocated mutex until source drop.
    drop((legacy, files));
    assert_eq!(probe.live(), 0);
    drop(probe);
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        binding
            .transport_goal_absent_admitted(&session, &policy)
            .unwrap_err()
    });
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.calls.load(SeqCst), 1);
    drop((probe, pressure, binding, source));
    root.assert_idle();
}

#[tokio::test]
async fn actual_bridge_transport_receipt_requires_owned_goal_absence_and_stable_status() {
    for fault in [0, 1] {
        let mut fixture = crate::hosted::acp::ingress::fixture::fixture();
        let (binding, source) = binding(
            &fixture.policy,
            fault,
            "conversation",
            output("conversation"),
            None,
        );
        fixture.bridge.context_refresh = Some(binding);
        fixture.activate().await;
        let result = fixture
            .bridge
            .managed_transport_receipt(7, &fixture.policy)
            .unwrap()
            .await
            .unwrap();
        assert_eq!(result.is_some(), fault == 0);
        assert_eq!(source.calls.load(SeqCst), 1);
        assert_eq!(
            source.status.calls.load(SeqCst),
            if fault == 0 { 3 } else { 2 }
        );
        let root = fixture.root.clone();
        drop(fixture);
        assert!(root.available_bytes() < root.capacity_bytes());
        drop((result, source));
        root.assert_idle();
    }
}

#[test]
fn malformed_goal_json_refuses_without_second_status_or_legacy_fallback() {
    let (root, policy) = policy();
    for bytes in [b"unknown output".to_vec(), vec![b'x'; 4 * 1024 * 1024 + 1]] {
        // Oversized producer storage is tested directly at the JSON boundary;
        // the small goal fixture intentionally cannot admit a 4 MiB source.
        let error = JsonFrame::decode_admitted(&bytes, &policy).unwrap_err();
        assert!(error.belongs_to(&policy));
        root.assert_idle();
    }
    let (binding, source) = binding(&policy, 0, "conversation", b"unknown output".to_vec(), None);
    assert!(binding
        .transport_goal_absent_admitted("conversation", &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .is_err());
    assert_eq!(source.status.calls.load(SeqCst), 1);
    drop((binding, source));
    root.assert_idle();
}

#[test]
fn strict_goal_inspection_preserves_status_errors_and_malformed_output_category() {
    let (root, policy) = policy();
    for fault in [0, 1, 5, 6] {
        let (binding, source) =
            binding(&policy, fault, "conversation", output("conversation"), None);
        let result = binding
            .transport_goal_absent_strict_admitted("conversation", &policy)
            .unwrap()
            .now_or_never()
            .unwrap();
        match fault {
            0 => assert!(**result.as_ref().unwrap()),
            1 => assert!(!**result.as_ref().unwrap()),
            5 => assert!(matches!(
                result.as_ref().unwrap_err().as_adapter(),
                Some(crate::AdapterError::Rejected(_))
            )),
            6 => assert!(matches!(
                result.as_ref().unwrap_err().as_adapter(),
                Some(crate::AdapterError::Unsupported(_))
            )),
            _ => unreachable!(),
        }
        assert_eq!(source.calls.load(SeqCst), usize::from(fault < 5));
        drop((result, binding, source));
        root.assert_idle();
    }
    let (binding, source) = binding(&policy, 0, "conversation", b"malformed".to_vec(), None);
    let error = binding
        .transport_goal_absent_strict_admitted("conversation", &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(matches!(error, AdmittedError::Unsupported(_)));
    assert_eq!(source.status.calls.load(SeqCst), 1);
    drop((binding, source));
    root.assert_idle();
}
