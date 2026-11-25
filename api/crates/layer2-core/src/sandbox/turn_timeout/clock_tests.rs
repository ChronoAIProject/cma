use super::{approval::*, *};

fn waiting(
    limits: ApprovalLimits,
    expiry: i64,
    observed: i64,
) -> (TurnTimeout, PendingApprovalReceipt) {
    let admitted = TurnTimeout::admit_approval(
        super::tests::binding(),
        uuid::Uuid::new_v4().to_string(),
        limits,
        ApprovalClockBounds {
            offset_min_ms: 0,
            offset_max_ms: 0,
            valid_until_ms: 901_000,
        },
        1_000,
    )
    .unwrap();
    let receipt = PendingApprovalReceipt {
        binding: ApprovalOperationBinding {
            operation_id: "first-operation".into(),
            model_attempt_id: "first-attempt".into(),
            provider: "external".into(),
            authority_digest: "c".repeat(64),
        },
        review_id: "first-review".into(),
        revision: 30,
        created_at_ms: 1_000,
        expires_at_ms: expiry,
    };
    let registered = admitted
        .register_approval_operation(admitted.revision(), receipt.binding.clone(), 1_000)
        .unwrap();
    let pending = registered
        .observe_pending_approval(registered.revision(), &receipt, observed)
        .unwrap();
    (pending, receipt)
}

fn reload(policy: &TurnTimeout) -> TurnTimeout {
    let value: TurnTimeout = serde_json::from_slice(&serde_json::to_vec(policy).unwrap()).unwrap();
    value.validate().unwrap();
    value
}

fn another(first: &ApprovalOperationBinding) -> ApprovalOperationBinding {
    ApprovalOperationBinding {
        operation_id: "second-operation".into(),
        model_attempt_id: "second-attempt".into(),
        ..first.clone()
    }
}

#[test]
fn duplicate_pending_preserves_observed_expiry_across_rollback_equality_and_reload() {
    for observed in [301_000, 351_000] {
        let (waiting, receipt) = waiting(ApprovalLimits::default(), 901_000, 101_000);
        let seen = waiting
            .observe_pending_approval(waiting.revision(), &receipt, observed)
            .unwrap();
        assert_eq!(seen.observed_at_ms, observed);
        assert_eq!(seen.revision(), waiting.revision() + 1);
        assert_eq!(seen.deadline_at_ms(), waiting.deadline_at_ms());
        assert_eq!(seen.waiting_since_ms(), waiting.waiting_since_ms());
        assert!(seen.approval_events_since(&waiting).unwrap().is_empty());
        let persisted = reload(&seen);
        for replay_time in [151_000, observed] {
            assert_eq!(
                persisted
                    .observe_pending_approval(persisted.revision(), &receipt, replay_time)
                    .unwrap(),
                persisted
            );
        }
        assert!(persisted
            .expire(persisted.revision(), 151_000)
            .unwrap()
            .blocks_dispatch());
    }
}

#[test]
fn duplicate_resolution_preserves_another_waits_observed_expiry() {
    let (waiting, receipt) = waiting(ApprovalLimits::default(), 901_000, 1_000);
    let second = another(&receipt.binding);
    let registered = waiting
        .register_approval_operation(waiting.revision(), second.clone(), 1_000)
        .unwrap();
    let both = registered
        .observe_pending_approval(
            registered.revision(),
            &PendingApprovalReceipt {
                binding: second,
                review_id: "second-review".into(),
                ..receipt.clone()
            },
            1_000,
        )
        .unwrap();
    let resolution = ResolvedApprovalReceipt {
        binding: receipt.binding,
        revision: 31,
        resolved_at_ms: 101_000,
        resolution: ApprovalResolution::Approved,
    };
    let approved = both
        .observe_approval_resolution(both.revision(), &resolution, 101_000)
        .unwrap();
    let seen = approved
        .observe_approval_resolution(approved.revision(), &resolution, 351_000)
        .unwrap();
    assert_eq!(seen.observed_at_ms, 351_000);
    assert!(seen.is_waiting_for_external_approval());
    assert!(seen.approval_events_since(&approved).unwrap().is_empty());
    let persisted = reload(&seen);
    assert_eq!(
        persisted
            .observe_approval_resolution(persisted.revision(), &resolution, 151_000)
            .unwrap(),
        persisted
    );
    assert!(persisted
        .expire(persisted.revision(), 151_000)
        .unwrap()
        .blocks_dispatch());
}

#[test]
fn replaying_existing_reservation_advances_time_without_admitting_a_new_effect() {
    let (waiting, receipt) = waiting(ApprovalLimits::default(), 901_000, 1_000);
    let seen = waiting
        .register_approval_operation(waiting.revision(), receipt.binding.clone(), 351_000)
        .unwrap();
    assert_eq!(seen.observed_at_ms, 351_000);
    assert!(seen.approval_events_since(&waiting).unwrap().is_empty());
    let persisted = reload(&seen);
    assert!(persisted
        .register_approval_operation(persisted.revision(), another(&receipt.binding), 151_000)
        .is_err());
    assert_eq!(
        persisted
            .register_approval_operation(persisted.revision(), receipt.binding, 151_000)
            .unwrap(),
        persisted
    );
    assert!(persisted
        .expire(persisted.revision(), 151_000)
        .unwrap()
        .blocks_dispatch());
}

#[test]
fn new_reservations_refuse_provider_episode_and_cumulative_deadline_including_equality() {
    for (limits, expiry) in [
        (ApprovalLimits::default(), 101_000),
        (
            ApprovalLimits {
                episode_ms: 100_000,
                total_ms: 300_000,
            },
            901_000,
        ),
        (
            ApprovalLimits {
                episode_ms: 300_000,
                total_ms: 100_000,
            },
            901_000,
        ),
    ] {
        let (waiting, receipt) = waiting(limits, expiry, 1_000);
        assert_eq!(waiting.deadline_at_ms(), 101_000);
        assert!(waiting
            .register_approval_operation(waiting.revision(), another(&receipt.binding), 100_999)
            .is_ok());
        for now in [101_000, 201_000] {
            assert_eq!(
                waiting.register_approval_operation(
                    waiting.revision(),
                    another(&receipt.binding),
                    now
                ),
                Err(TurnTimeoutError::Conflict)
            );
            assert!(waiting
                .register_approval_operation(waiting.revision(), receipt.binding.clone(), now)
                .is_ok());
            assert!(waiting
                .observe_pending_approval(waiting.revision(), &receipt, now)
                .is_ok());
        }
    }
}
