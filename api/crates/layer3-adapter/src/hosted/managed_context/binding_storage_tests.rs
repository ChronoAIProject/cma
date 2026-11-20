use super::binding::ContextInput;
use super::*;
use crate::allocation_probe::{deallocation, AllocationProbe};
use crate::ingress_budget::{AdmissionRefusal, AllocationPlan, IngressClass};
use futures_util::FutureExt;

struct Input {
    handle: RuntimeHandle,
    identity: RuntimeStatusRequest,
    cwd: String,
    tools: Vec<serde_json::Value>,
}

impl Input {
    fn new() -> Self {
        Self {
            handle: RuntimeHandle("r".repeat(32 * 1024)),
            identity: RuntimeStatusRequest {
                sandbox_id: "s".repeat(256),
                generation: 3,
                home_identity: "h".repeat(43),
            },
            cwd: "c".repeat(32 * 1024),
            tools: vec![
                serde_json::json!({"name":"synthetic", "nested":[{"value":"v".repeat(32 * 1024)}]}),
            ],
        }
    }
    fn borrowed(&self) -> ContextInput<'_> {
        ContextInput {
            handle: &self.handle,
            identity: &self.identity,
            home: "/home/cma/.cma/agents/synthetic/home",
            cwd: &self.cwd,
            tools: &self.tools,
        }
    }
}

#[test]
fn initial_context_storage_remains_owned_between_admitted_operations() {
    const TEST: &str = "hosted::managed_context::binding_storage_tests::initial_context_storage_remains_owned_between_admitted_operations";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = guidance_admission::fixture::policy();
    let mut probe = AllocationProbe::new();
    let (binding, source) = probe.track(|| receipt_admission::tests::binding(&policy, 0, None));
    let retained = binding.clone();
    let receipt = binding
        .readiness_receipt_admitted("synthetic-session", 7, &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    assert!(receipt.is_some());
    drop((receipt, binding, source));
    assert!(probe.live() > 0);
    assert!(
        probe.live() <= root.reserved_work_bytes(),
        "initial context storage must retain admission between operations"
    );
    drop(retained);
    assert_eq!(probe.live(), 0);
    root.assert_idle();
}

#[test]
fn construction_refuses_pressure_and_foreign_sources_before_copying() {
    let (root, policy) = guidance_admission::fixture::policy();
    let (base, source) = receipt_admission::tests::binding(&policy, 0, None);
    let input = Input::new();
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("materialized context pressure", &[available]).unwrap(),
        )
        .unwrap();
    let payload = vec![0u8; available];
    let mut probe = AllocationProbe::new();
    let result = probe.track(|| {
        HostedContextRefresh::new_admitted(
            &policy,
            binding_fixture::sources(&base),
            input.borrowed(),
        )
    });
    assert!(matches!(
        result.unwrap_err().refusal(),
        Some(AdmissionRefusal::AtCapacity { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    let (foreign_root, foreign) = guidance_admission::fixture::policy();
    let result = probe.track(|| {
        HostedContextRefresh::new_admitted(
            &foreign,
            binding_fixture::sources(&base),
            input.borrowed(),
        )
    });
    assert!(matches!(
        result.unwrap_err().refusal(),
        Some(AdmissionRefusal::InvalidPlan { .. })
    ));
    assert_eq!(probe.allocated(), 0);
    drop((payload, pressure, base, source));
    root.assert_idle();
    foreign_root.assert_idle();
}

#[test]
fn nested_context_copies_and_shared_clones_keep_actual_storage() {
    const TEST: &str = "hosted::managed_context::binding_storage_tests::nested_context_copies_and_shared_clones_keep_actual_storage";
    if deallocation::isolated(TEST) {
        return;
    }
    let (root, policy) = guidance_admission::fixture::policy();
    let (base, source) = receipt_admission::tests::binding(&policy, 0, None);
    let input = Input::new();
    let baseline = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let context = probe.track(|| {
        HostedContextRefresh::new_admitted(
            &policy,
            binding_fixture::sources(&base),
            input.borrowed(),
        )
        .unwrap()
    });
    assert!(probe.peak() >= 3 * 32 * 1024);
    assert!(probe.peak() <= baseline - root.available_bytes());
    assert_eq!(context.0.tools, input.tools);
    assert_eq!(context.0.handle, input.handle);
    assert_eq!(context.0.identity, input.identity);
    assert_eq!(context.cwd(), input.cwd);
    let allocated = probe.allocated();
    let retained = probe.track(|| context.clone());
    assert_eq!(probe.allocated(), allocated);
    assert_eq!(context, retained);
    assert_eq!(context.0.tools.as_ptr(), retained.0.tools.as_ptr());
    assert_ne!(context, base);
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        drop(context);
        panic!("synthetic context startup failed");
    }));
    assert!(panic.is_err());
    assert!(probe.live() <= baseline - root.available_bytes());
    drop(retained);
    assert_eq!(probe.live(), 0);
    assert_eq!(root.available_bytes(), baseline);
    drop((base, source));
    root.assert_idle();
}

#[test]
fn validation_errors_keep_their_categories_and_storage_through_last_clone() {
    let (root, policy) = guidance_admission::fixture::policy();
    let (base, source) = receipt_admission::tests::binding(&policy, 0, None);
    let mut input = Input::new();
    let baseline = root.available_bytes();
    for fault in 0..3 {
        input.identity.generation = if fault == 1 { 0 } else { 3 };
        if fault == 2 {
            input.tools = vec![serde_json::Value::Null; 8193];
        }
        let mut borrowed = input.borrowed();
        if fault == 0 {
            borrowed.home = "/invalid";
        }
        let mut probe = AllocationProbe::new();
        let error = probe.track(|| {
            HostedContextRefresh::new_admitted(&policy, binding_fixture::sources(&base), borrowed)
                .unwrap_err()
        });
        assert!(error.belongs_to(&policy));
        assert!(matches!(
            (fault, error.as_adapter()),
            (0, Some(AdapterError::Unsupported(_))) | (1 | 2, Some(AdapterError::Rejected(_)))
        ));
        assert!(probe.live() > 0);
        assert!(probe.peak() <= baseline - root.available_bytes());
        let clone = probe.track(|| error.clone());
        drop(error);
        assert!(probe.live() > 0);
        drop(clone);
        assert_eq!(probe.live(), 0);
        assert_eq!(root.available_bytes(), baseline);
    }
    drop((base, source));
    root.assert_idle();
}

#[test]
fn isolated_context_custody_survives_final_physical_deallocation() {
    const TEST: &str = "hosted::managed_context::binding_storage_tests::isolated_context_custody_survives_final_physical_deallocation";
    if deallocation::isolated(TEST) {
        return;
    }
    for concurrent in [false, true] {
        let (root, policy) = guidance_admission::fixture::policy();
        let (base, source) = receipt_admission::tests::binding(&policy, 0, None);
        let input = Input::new();
        let baseline = root.available_bytes();
        let mut probe = AllocationProbe::new();
        let (context, pause) = probe.pause_deallocation_containing(
            deallocation::arc_allocation_bytes::<Binding>(),
            || {
                HostedContextRefresh::new_admitted(
                    &policy,
                    binding_fixture::sources(&base),
                    input.borrowed(),
                )
                .unwrap()
            },
            |context| &*context.0 as *const Binding as usize,
        );
        let charged = baseline - root.available_bytes();
        let clone = context.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        pause.inspect_while_dropping(
            move || {
                if concurrent {
                    let peer = barrier.clone();
                    let worker = std::thread::spawn(move || {
                        peer.wait();
                        drop(context);
                    });
                    barrier.wait();
                    drop(clone);
                    worker.join().unwrap();
                } else {
                    drop((context, clone));
                }
            },
            || {
                assert_eq!(root.available_bytes(), baseline - charged);
                assert!(policy
                    .admit(
                        IngressClass::Data,
                        AllocationPlan::checked(
                            "pre-free context reuse",
                            &[policy.budget(IngressClass::Data).available_bytes() + 1],
                        )
                        .unwrap()
                    )
                    .is_err());
            },
        );
        assert_eq!(root.available_bytes(), baseline);
        assert_eq!(probe.live(), 0);
        drop((base, source));
        root.assert_idle();
    }
}

#[test]
fn legacy_context_refuses_admitted_operations_before_future_or_source_allocation() {
    let (root, policy) = guidance_admission::fixture::policy();
    let (legacy, files) = writer_tests::binding(0);
    let mut probe = AllocationProbe::new();
    probe.track(|| {
        assert!(legacy
            .readiness_receipt_admitted("conversation", 1, &policy)
            .unwrap_err()
            .refusal()
            .is_some());
        assert!(legacy
            .confirm_instructions_admitted("instructions", &policy)
            .unwrap_err()
            .refusal()
            .is_some());
        assert!(legacy
            .append_guidance_admitted("conversation", 1, "receipt", "guidance", &[], &policy)
            .unwrap_err()
            .refusal()
            .is_some());
        assert!(legacy
            .transport_goal_absent_admitted("conversation", &policy)
            .unwrap_err()
            .refusal()
            .is_some());
        assert!(legacy
            .prepare_transport_admitted("conversation", 1, None, &policy)
            .now_or_never()
            .unwrap()
            .err()
            .unwrap()
            .refusal()
            .is_some());
    });
    assert_eq!(probe.allocated(), 0);
    assert_eq!(
        files.status_reads.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(files.writes.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert_eq!(
        files.goal_probes.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    root.assert_idle();
}
