use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy, SharedOwned,
};
use futures_util::FutureExt;
use std::sync::atomic::Ordering::SeqCst;

#[test]
fn erased_backend_and_binding_keep_exact_attachment_custody() {
    let root = IngressBudget::new(4 * 1024 * 1024);
    let floor = AllocationPlan::checked("context attachment floor", &[1024 * 1024]).unwrap();
    let bounds = ClassBounds::new(root.capacity_bytes(), floor, floor)
        .unwrap()
        .with_attachment_limit(2)
        .unwrap();
    let policy = IngressOwnershipPolicy::new_admitted(root.clone(), bounds).unwrap();
    let first = policy.admit_attachment().unwrap();
    let second = policy.admit_attachment().unwrap();
    let (context, source) = receipt_admission::tests::binding(first.policy(), 0, None);
    let mut probe = AllocationProbe::new();
    let erased: SharedOwned<dyn HostedBackend> = probe.track(|| source.clone().into());
    assert_eq!(probe.allocated(), 0);
    assert_eq!(
        &*source as *const _ as *const (),
        &*erased as *const dyn HostedBackend as *const ()
    );
    probe.track(|| {
        assert!(context.require_owner(second.policy()).is_err());
        assert!(context
            .readiness_receipt_admitted("conversation", 1, second.policy())
            .unwrap_err()
            .refusal()
            .is_some());
        let input = binding::ContextInput {
            handle: &context.0.handle,
            identity: &context.0.identity,
            home: "/home/cma/.cma/agents/synthetic/home",
            cwd: context.cwd(),
            tools: &[],
        };
        assert!(HostedContextRefresh::new_admitted(
            second.policy(),
            binding_fixture::sources(&context),
            input
        )
        .unwrap_err()
        .refusal()
        .is_some());
    });
    assert_eq!(probe.allocated(), 0);
    assert_eq!(source.calls.load(SeqCst), 0);
    drop((first, source, erased));
    assert!(policy.admit_attachment().is_err());
    drop(context);
    drop(policy.admit_attachment().unwrap());
    drop(second);
    root.assert_idle();
}

#[test]
fn context_rejects_foreign_optional_source_and_binding_owners() {
    for provider in [false, true] {
        let (root, policy) = guidance_admission::fixture::policy();
        let (foreign_root, foreign) = guidance_admission::fixture::policy();
        let (context, source) = transport_admission::tests::binding(&policy, provider, 0, None);
        let (other, other_source) =
            transport_admission::tests::binding(&foreign, provider, 0, None);
        for source_only in [false, true] {
            let mut sources = binding_fixture::sources(&context);
            let mut alien = binding_fixture::sources(&other);
            if provider {
                let local = sources.provider.as_mut().unwrap();
                let alien = alien.provider.take().unwrap();
                if source_only {
                    local.0 = alien.0;
                } else {
                    local.1 = alien.1;
                }
            } else {
                let local = sources.access.as_mut().unwrap();
                let alien = alien.access.take().unwrap();
                if source_only {
                    local.0 = alien.0;
                } else {
                    local.1 = alien.1;
                }
            }
            let input = binding::ContextInput {
                handle: &context.0.handle,
                identity: &context.0.identity,
                home: "/home/cma/.cma/agents/synthetic/home",
                cwd: context.cwd(),
                tools: &context.0.tools,
            };
            let mut probe = AllocationProbe::new();
            let error = probe
                .track(|| HostedContextRefresh::new_admitted(&policy, sources, input).unwrap_err());
            assert!(error.refusal().is_some());
            assert_eq!(probe.allocated(), 0);
        }
        drop((context, source, other, other_source));
        root.assert_idle();
        foreign_root.assert_idle();
    }
}

#[tokio::test]
async fn retained_context_remains_callable_after_bridge_teardown() {
    let mut fixture = crate::hosted::acp::ingress::fixture::fixture();
    let root = fixture.root.clone();
    let policy = fixture.policy.clone();
    let (context, source) = receipt_admission::tests::binding(&policy, 0, None);
    fixture.bridge.context_refresh = Some(context.clone());
    drop(fixture);
    assert!(root.reserved_work_bytes() > 0);
    let receipt = context
        .readiness_receipt_admitted("conversation", 1, &policy)
        .unwrap()
        .now_or_never()
        .unwrap()
        .unwrap();
    assert_eq!(source.calls.load(SeqCst), 1);
    assert!(receipt.is_some());
    drop((context, source));
    assert!(root.reserved_work_bytes() > 0);
    drop(receipt);
    root.assert_idle();
}

#[test]
fn main_storage_refusal_releases_validation_before_any_identity_or_tool_copy() {
    let (root, policy) = guidance_admission::fixture::policy();
    let (context, source) = receipt_admission::tests::binding(&policy, 0, None);
    let cwd = "c".repeat(32 * 1024);
    let available = policy.budget(IngressClass::Data).available_bytes();
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked("binding copy pressure", &[available - 4096]).unwrap(),
        )
        .unwrap();
    let payload = vec![0u8; available - 4096];
    let baseline = root.available_bytes();
    let mut probe = AllocationProbe::new();
    let error = probe.track(|| {
        HostedContextRefresh::new_admitted(
            &policy,
            binding_fixture::sources(&context),
            binding::ContextInput {
                handle: &context.0.handle,
                identity: &context.0.identity,
                home: "/home/cma/.cma/agents/synthetic/home",
                cwd: &cwd,
                tools: &[],
            },
        )
        .unwrap_err()
    });
    assert!(matches!(
        error.refusal(),
        Some(crate::ingress_budget::AdmissionRefusal::AtCapacity {
            phase: "initial context binding"
        })
    ));
    assert_eq!(probe.allocated(), 0);
    assert_eq!(root.available_bytes(), baseline);
    drop((payload, pressure, context, source));
    root.assert_idle();
}
