use super::*;
use crate::allocation_probe::AllocationProbe;
use crate::ingress_budget::{
    sizing, AllocationPlan, ClassBounds, IngressBudget, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope,
};
use futures_util::FutureExt;

type Category = fn(String) -> AdapterError;

fn cases() -> Vec<(AdapterError, Category)> {
    let mut cases: Vec<_> = [
        AdapterError::PromptNotWritten as Category,
        AdapterError::InputAuthorityUnavailable,
        AdapterError::Unavailable,
        AdapterError::Rejected,
        AdapterError::OperationIdentityConflict,
        AdapterError::RuntimeLedgerHold,
        AdapterError::AtCapacity,
        AdapterError::Unsupported,
    ]
    .into_iter()
    .map(|category| (category("untrusted echoed descriptor".into()), category))
    .collect();
    cases.extend([
        (
            AdapterError::RuntimeStartupHold(crate::RuntimeStartupFailure {
                class: crate::RuntimeStartupFailureClass::Terminated,
                consecutive: 3,
            }),
            AdapterError::Unavailable as Category,
        ),
        (
            AdapterError::SkillUpgradeRefused(crate::AgentSkillUpgradeRefusal::Modified),
            AdapterError::Rejected as Category,
        ),
        (
            AdapterError::RuntimeContractMismatch(crate::RuntimeContractMismatch {
                image: "synthetic-image".into(),
                expected: "3".into(),
                found: Some("2".into()),
            }),
            AdapterError::Rejected as Category,
        ),
    ]);
    cases
}

#[test]
fn protected_error_formatting_preserves_redaction_and_category_under_pressure() {
    let plan = |bytes| AllocationPlan::checked("protected failure fixture", &[bytes]).unwrap();
    for pressure in [false, true] {
        for (source, category) in cases() {
            let root = IngressBudget::new(64 * 1024);
            let policy = IngressOwnershipPolicy::new(
                root.clone(),
                ClassBounds::new(root.capacity_bytes(), plan(4096), plan(4096)).unwrap(),
            )
            .unwrap();
            let scope = IngressWorkScope::new(
                policy
                    .admit(IngressClass::Data, plan(sizing::shared_owner_bytes::<()>()))
                    .unwrap(),
            );
            let mut probe = AllocationProbe::new();
            let error = scope
                .initialize_owned(|| async {
                    let _pressure = pressure.then(|| {
                        let available = policy.budget(IngressClass::Data).available_bytes();
                        policy
                            .admit(IngressClass::Data, plan(available))
                            .unwrap()
                            .initialize(|| vec![0u8; available - 1024])
                    });
                    Err::<(), _>(probe.track(|| protected_setup_failure(source, "permission_mode")))
                })
                .resolve_admitted_result(&policy, IngressClass::Data)
                .now_or_never()
                .unwrap()
                .unwrap_err();
            let expected = if pressure {
                String::new()
            } else {
                "channel tool session setup did not complete (phase: permission_mode)".into()
            };
            assert_eq!(error.as_adapter(), Some(&category(expected)));
            if pressure {
                assert_eq!(
                    probe.allocated(),
                    0,
                    "redaction refusal must allocate no text"
                );
            }
            drop((error, scope));
            assert_eq!(probe.live(), 0);
            root.assert_idle();
        }
    }
}

#[test]
fn protected_legacy_errors_keep_the_exact_phase_and_special_mapping() {
    for (error, category) in cases() {
        assert_eq!(
            protected_setup_failure(error, "session_load"),
            category("channel tool session setup did not complete (phase: session_load)".into())
        );
    }
    let guarded =
        AdapterError::NativeClaimNotReady(crate::agent_sandbox::NativeClaimGuardReason::Pending);
    assert_eq!(
        protected_setup_failure(guarded.clone(), "session_load"),
        guarded
    );
}
