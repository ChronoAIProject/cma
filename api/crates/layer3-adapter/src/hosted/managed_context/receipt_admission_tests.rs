use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{ClassBounds, IngressBudget, OwnedWork};
use crate::wire::{AgentOpenReceipt, FencedReceipt, FencedRequest};
use crate::{AgentLaunch, ConversationChannel, ConversationUpdate, RuntimeObservation};
use futures_util::FutureExt;
use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

pub(in crate::hosted) struct Source {
    pub(in crate::hosted) calls: AtomicUsize,
    fault: u8,
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
        assert_eq!(handle.as_str(), "synthetic-runtime");
        if !request.belongs_to(policy) {
            return Err(AdmissionRefusal::InvalidPlan {
                phase: "synthetic request owner",
            }
            .into());
        }
        let RequestFrame::RuntimeStatus(identity) = &*request else {
            panic!("unexpected request")
        };
        let plan = AllocationPlan::checked(
            "synthetic status strings",
            &[
                identity.sandbox_id.len(),
                identity.home_identity.len(),
                32,
                "synthetic source failure".len(),
            ],
        )?;
        AdmittedOperation::new(
            self.foreign.as_ref().unwrap_or(policy),
            IngressClass::Data,
            plan,
            move |_| async move {
                self.calls.fetch_add(1, SeqCst);
                if self.fault == 4 {
                    std::future::pending::<()>().await;
                }
                if self.fault == 3 {
                    return Err(AdapterError::Unavailable("synthetic source failure".into()));
                }
                let RequestFrame::RuntimeStatus(identity) = &*request else {
                    unreachable!()
                };
                Ok(ResponseFrame::RuntimeStatus(RuntimeStatus {
                    agent_browser: false,
                    agent_filesystem: true,
                    agent_attachment_upload: false,
                    agent_skill_install: false,
                    agent_skill_upgrade: false,
                    agent_model_catalog: false,
                    agent_resource_preparation: false,
                    agent_process_restart: false,
                    exact_github_credential_route: false,
                    github_repository_scope: false,
                    sandbox_id: identity.sandbox_id.clone(),
                    generation: identity.generation + u64::from(self.fault == 1),
                    home_identity: identity.home_identity.clone(),
                    boot_id: "a".repeat(32),
                    installation_ready: true,
                    credential_ready: self.fault != 2,
                    restore_pending: false,
                    agent_processes: 1,
                }))
            },
        )
    }

    async fn exchange(&self, _: &RuntimeHandle, _: RequestFrame) -> crate::Result<ResponseFrame> {
        panic!("admitted status cannot invoke the legacy producer")
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
    policy: &IngressOwnershipPolicy,
    fault: u8,
    foreign: Option<IngressOwnershipPolicy>,
) -> (
    HostedContextRefresh,
    crate::ingress_budget::SharedOwned<Source>,
) {
    let source = super::super::binding_fixture::source(policy, 0, || Source {
        calls: AtomicUsize::new(0),
        fault,
        foreign,
    });
    let binding = super::super::binding_fixture::context(policy, source.clone().into());
    (binding, source)
}

fn policy() -> (IngressBudget, IngressOwnershipPolicy) {
    let root = IngressBudget::new(128 * 1024);
    let floor = AllocationPlan::checked("receipt fixture floor", &[16384]).unwrap();
    let policy = IngressOwnershipPolicy::new(
        root.clone(),
        ClassBounds::new(root.capacity_bytes(), floor, floor).unwrap(),
    )
    .unwrap();
    // Initialize the shared policy's semaphore backing before source measurement.
    drop(
        policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked("policy backing", &[1]).unwrap(),
            )
            .unwrap(),
    );
    (root, policy)
}

#[test]
fn source_receipt_keeps_storage_and_rejects_stale_or_unready_status() {
    if crate::allocation_probe::deallocation::isolated(
        "hosted::managed_context::receipt_admission::tests::source_receipt_keeps_storage_and_rejects_stale_or_unready_status",
    ) {
        return;
    }
    for fault in 0..=2 {
        let (root, policy) = policy();
        let (binding, source) = binding(&policy, fault, None);
        let mut probe = AllocationProbe::new();
        let result = probe.track(|| {
            binding
                .readiness_receipt_admitted("synthetic-session", 7, &policy)
                .unwrap()
                .now_or_never()
                .unwrap()
                .unwrap()
        });
        assert_eq!(source.calls.load(SeqCst), 1);
        assert!(result.belongs_to(&policy));
        assert_eq!(result.is_some(), fault == 0);
        assert!(probe.live() <= root.reserved_work_bytes());
        if fault == 0 {
            assert_eq!(result.as_ref().unwrap().len(), 64);
            assert!(probe.live() >= 64);
            let legacy = serde_json::json!([
                "managed-context-v1",
                "synthetic-runtime",
                "synthetic-sandbox",
                3,
                "h".repeat(43),
                "a".repeat(32),
                "cma/.cma/agents/synthetic/home/AGENTS.md",
                "synthetic-session",
                7,
            ]);
            let expected = format!("{:x}", Sha256::digest(serde_json::to_vec(&legacy).unwrap()));
            assert_eq!(result.as_deref(), Some(expected.as_str()));
        }
        drop((binding, source, policy));
        assert!(root.available_bytes() < root.capacity_bytes());
        drop(result);
        assert_eq!(probe.live(), 0);
        root.assert_idle();
    }
}

#[test]
fn source_error_retains_custody_and_foreign_operation_is_never_polled() {
    let (root, policy) = policy();
    let (binding, source) = binding(&policy, 3, None);
    let error = binding
        .readiness_receipt_admitted("synthetic-session", 7, &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(
        matches!(error.as_adapter(), Some(AdapterError::Unavailable(text)) if text == "synthetic source failure")
    );
    assert!(error.belongs_to(&policy));
    assert_eq!(source.calls.load(SeqCst), 1);
    drop((binding, source));
    assert!(root.available_bytes() < root.capacity_bytes());
    drop(error);
    root.assert_idle();

    let (foreign_root, foreign) = self::policy();
    let (binding, source) = self::binding(&policy, 0, Some(foreign));
    let error = binding
        .readiness_receipt_admitted("synthetic-session", 7, &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap_err();
    assert!(matches!(
        error.refusal(),
        Some(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(source.calls.load(SeqCst), 0);
    drop((binding, source));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[test]
fn status_admission_precedes_io_and_cancellation_releases_request_and_future() {
    let (root, policy) = policy();
    let (binding, source) = binding(&policy, 4, None);
    let baseline = root.available_bytes();
    let mut operation = Box::pin(
        binding
            .readiness_receipt_admitted("synthetic-session", 7, &policy)
            .unwrap(),
    );
    assert!(operation.as_mut().now_or_never().is_none());
    assert_eq!(source.calls.load(SeqCst), 1);
    drop(operation);
    assert_eq!(root.available_bytes(), baseline);
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "receipt pressure",
                &[policy.budget(IngressClass::Data).available_bytes()],
            )
            .unwrap(),
        )
        .unwrap();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        binding
            .readiness_receipt_admitted("synthetic-session", 7, &policy)
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
