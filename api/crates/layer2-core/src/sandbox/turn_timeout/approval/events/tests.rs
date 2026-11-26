use super::*;
use crate::sandbox::turn_timeout::tests::binding;

fn pending() -> (TurnTimeout, TurnTimeout, PendingApprovalReceipt) {
    let admitted = TurnTimeout::admit_approval(
        binding(),
        uuid::Uuid::new_v4().to_string(),
        ApprovalLimits::default(),
        ApprovalClockBounds {
            offset_min_ms: 0,
            offset_max_ms: 0,
            valid_until_ms: 901_000,
        },
        1_000,
    )
    .unwrap();
    let operation = ApprovalOperationBinding {
        operation_id: "private-operation".into(),
        model_attempt_id: "private-attempt".into(),
        provider: "external".into(),
        authority_digest: "c".repeat(64),
    };
    let registered = admitted
        .register_approval_operation(1, operation.clone(), 1_000)
        .unwrap();
    assert!(registered
        .approval_events_since(&admitted)
        .unwrap()
        .is_empty());
    let receipt = PendingApprovalReceipt {
        binding: operation,
        review_id: "private-review".into(),
        revision: 30,
        created_at_ms: 1_000,
        expires_at_ms: 901_000,
    };
    let pending = registered
        .observe_pending_approval(registered.revision(), &receipt, 2_000)
        .unwrap();
    (registered, pending, receipt)
}

#[test]
fn public_wait_is_opaque_stable_and_bound_to_its_original_turn() {
    let (registered, waiting, receipt) = pending();
    let events = waiting.approval_events_since(&registered).unwrap();
    let [AgentLogKind::ExternalApprovalRequested {
        approval_id,
        turn_sequence,
        waiting_since,
        deadline_at,
        revision,
        ..
    }] = &events[..]
    else {
        panic!("missing requested event")
    };
    assert!(approval_id.starts_with("external_"));
    assert_eq!(*turn_sequence, binding().turn_sequence);
    assert_eq!(*revision, waiting.revision());
    assert_eq!(waiting_since, "1970-01-01T00:00:01.000Z");
    assert_eq!(deadline_at, "1970-01-01T00:05:01.000Z");
    let json = serde_json::to_string(&events).unwrap();
    for private in [
        "private-operation",
        "private-attempt",
        "private-review",
        &"c".repeat(64),
    ] {
        assert!(!json.contains(private));
    }
    let mut alias = receipt.clone();
    alias.revision += 1;
    alias.review_id = "private-replacement".into();
    alias.created_at_ms += 10_000;
    alias.expires_at_ms += 10_000;
    let aliased = waiting
        .observe_pending_approval(waiting.revision(), &alias, 12_000)
        .unwrap();
    assert!(aliased.approval_events_since(&waiting).unwrap().is_empty());
    assert_eq!(
        views(&waiting).unwrap()[0].id,
        views(&aliased).unwrap()[0].id
    );
    let mut other = registered.clone();
    other.binding.creation_key.push_str("-another-incarnation");
    let other_wait = other
        .observe_pending_approval(other.revision(), &receipt, 2_000)
        .unwrap();
    assert_ne!(views(&other_wait).unwrap()[0].id, *approval_id);
    assert!(other_wait.approval_events_since(&waiting).is_err());
}

#[test]
fn public_wait_times_preserve_exact_canonical_milliseconds() {
    for (ms, expected) in [
        (1_000, "1970-01-01T00:00:01.000Z"),
        (1_001, "1970-01-01T00:00:01.001Z"),
        (1_010, "1970-01-01T00:00:01.010Z"),
        (1_100, "1970-01-01T00:00:01.100Z"),
        (1_120, "1970-01-01T00:00:01.120Z"),
        (1_123, "1970-01-01T00:00:01.123Z"),
        (-1, "1969-12-31T23:59:59.999Z"),
    ] {
        assert_eq!(timestamp(ms).unwrap(), expected);
    }
}

#[test]
fn timeout_or_native_winner_resolves_each_wait_without_a_native_action() {
    let (_, waiting, receipt) = pending();
    for (terminal, expected) in [
        (
            waiting.expire(waiting.revision(), 301_000).unwrap(),
            ExternalApprovalResolution::TimedOut,
        ),
        (
            waiting.supersede(waiting.revision()).unwrap(),
            ExternalApprovalResolution::Cancelled,
        ),
    ] {
        let events = terminal.approval_events_since(&waiting).unwrap();
        assert!(
            matches!(&events[..], [AgentLogKind::ExternalApprovalResolved { revision, resolution, .. }]
            if *revision > waiting.revision() && *resolution == expected)
        );
        assert!(!terminal.is_waiting_for_external_approval());
        // Closing the user-visible wait proves neither remote disposition nor
        // native quiescence, including when Stop or a result won locally.
        assert!(terminal.blocks_dispatch());
        assert!(terminal
            .approval_events_since(&terminal)
            .unwrap()
            .is_empty());
        assert!(terminal
            .observe_pending_approval(terminal.revision(), &receipt, 302_000)
            .is_err());
    }
}

#[test]
fn provider_resolution_stays_final_through_later_terminal_and_restart() {
    let (_, waiting, receipt) = pending();
    let approved = waiting
        .observe_approval_resolution(
            waiting.revision(),
            &ResolvedApprovalReceipt {
                binding: receipt.binding,
                revision: 31,
                resolved_at_ms: 101_000,
                resolution: ApprovalResolution::Approved,
            },
            101_000,
        )
        .unwrap();
    assert!(matches!(
        &approved.approval_events_since(&waiting).unwrap()[..],
        [AgentLogKind::ExternalApprovalResolved {
            resolution: ExternalApprovalResolution::Approved,
            ..
        }]
    ));
    let restarted: TurnTimeout =
        serde_json::from_slice(&serde_json::to_vec(&approved).unwrap()).unwrap();
    assert!(restarted
        .approval_events_since(&approved)
        .unwrap()
        .is_empty());
    assert!(!restarted.is_waiting_for_external_approval());
    assert!(restarted
        .supersede(restarted.revision())
        .unwrap()
        .blocks_dispatch());
    assert!(restarted
        .supersede(restarted.revision())
        .unwrap()
        .approval_events_since(&approved)
        .unwrap()
        .is_empty());
}

#[test]
fn one_denial_cancels_other_waits_without_mislabeling_them_as_timeouts() {
    let (_, waiting, first) = pending();
    let mut second = first.clone();
    second.binding.operation_id = "second-operation".into();
    second.binding.model_attempt_id = "second-attempt".into();
    second.created_at_ms = 2_000;
    let registered = waiting
        .register_approval_operation(waiting.revision(), second.binding.clone(), 2_000)
        .unwrap();
    let both = registered
        .observe_pending_approval(registered.revision(), &second, 2_000)
        .unwrap();
    let denied = both
        .observe_approval_resolution(
            both.revision(),
            &ResolvedApprovalReceipt {
                binding: first.binding,
                revision: 31,
                resolved_at_ms: 3_000,
                resolution: ApprovalResolution::Denied,
            },
            3_000,
        )
        .unwrap();
    let events = denied.approval_events_since(&both).unwrap();
    assert!(matches!(
        &events[..],
        [AgentLogKind::ExternalApprovalResolved {
            resolution: ExternalApprovalResolution::Denied,
            ..
        }]
    ));
    let terminal = denied.expire(denied.revision(), 3_000).unwrap();
    assert!(matches!(
        &terminal.approval_events_since(&denied).unwrap()[..],
        [AgentLogKind::ExternalApprovalResolved {
            resolution: ExternalApprovalResolution::Cancelled,
            ..
        }]
    ));
    assert!(terminal.blocks_dispatch());
}
