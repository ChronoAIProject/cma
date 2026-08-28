//! Cross-runtime usage accounting from the durable event log.

use std::num::NonZeroU64;

use super::{fold_usage, Usage};
use crate::event::EventKind;
use crate::session::SessionStatus;

fn marker(generation: u64) -> EventKind {
    EventKind::RuntimeStarted {
        generation: NonZeroU64::new(generation).expect("test generations are positive"),
    }
}

fn handover() -> EventKind {
    EventKind::RuntimeHandoverStarted
}

fn incarnation() -> EventKind {
    EventKind::RuntimeIncarnationStarted
}

fn status(status: SessionStatus, detail: Option<&str>) -> EventKind {
    EventKind::StatusChanged {
        status,
        stop_reason: None,
        detail: detail.map(str::to_owned),
        capability: None,
    }
}

fn report(
    input_tokens: u64,
    output_tokens: u64,
    context_used: u64,
    context_window: Option<u64>,
) -> EventKind {
    EventKind::UsageReported {
        input_tokens,
        output_tokens,
        context_used,
        context_window,
    }
}

fn usage(kinds: &[EventKind]) -> Usage {
    fold_usage(kinds.iter())
}

#[test]
fn legacy_and_replacement_runtime_maxima_are_summed() {
    let total = usage(&[
        report(100_000, 50_000, 80_000, Some(200_000)),
        EventKind::StatusChanged {
            status: SessionStatus::Rotating,
            stop_reason: None,
            detail: Some("moving onto the replacement runtime".into()),
            capability: None,
        },
        report(10_000, 5_000, 9_000, Some(200_000)),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (110_000, 55_000));
}

#[test]
fn an_unmarked_incarnation_uses_maxima_even_when_reports_dip() {
    let total = usage(&[
        report(100_000, 50_000, 90_000, Some(200_000)),
        report(70_000, 12_000, 40_000, Some(200_000)),
        report(95_000, 45_000, 60_000, Some(200_000)),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (100_000, 50_000));
}

#[test]
fn three_runtime_generations_each_contribute_their_maxima() {
    let total = usage(&[
        marker(1),
        report(10, 4, 10, None),
        marker(2),
        report(20, 8, 20, None),
        marker(3),
        report(30, 12, 30, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (60, 24));
}

#[test]
fn every_compatibility_incarnation_marker_closes_the_current_maxima() {
    let total = usage(&[
        incarnation(),
        report(10, 4, 10, None),
        incarnation(),
        report(20, 8, 20, None),
        incarnation(),
        report(30, 12, 30, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (60, 24));
}

#[test]
fn legacy_segments_and_exact_markers_compose_without_double_counting() {
    let total = usage(&[
        report(100, 50, 90, None),
        EventKind::StatusChanged {
            status: SessionStatus::Rotating,
            stop_reason: None,
            detail: Some("legacy handover".into()),
            capability: None,
        },
        report(10, 5, 40, None),
        EventKind::StatusChanged {
            status: SessionStatus::Ready,
            stop_reason: None,
            detail: None,
            capability: None,
        },
        report(20, 8, 60, None),
        handover(),
        status(SessionStatus::Rotating, Some("marker-aware handover")),
        marker(3),
        report(7, 3, 30, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (127, 61));
}

#[test]
fn duplicate_older_and_empty_generation_markers_do_not_inflate_usage() {
    let total = usage(&[
        report(100, 50, 100, None),
        marker(4),
        report(10, 5, 10, None),
        marker(4),
        report(20, 8, 20, None),
        marker(3),
        marker(5),
        marker(6),
        report(7, 3, 7, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (127, 61));
}

#[test]
fn a_marker_aware_rotation_status_has_no_arithmetic_meaning() {
    let total = usage(&[
        marker(1),
        report(100, 50, 90, None),
        EventKind::StatusChanged {
            status: SessionStatus::Rotating,
            stop_reason: None,
            detail: None,
            capability: None,
        },
        report(110, 55, 40, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (110, 55));
    assert_eq!(total.context_used, 40);
}

#[test]
fn a_coordinated_rollback_merges_the_continued_counter_with_or_without_detail() {
    for detail in [None, Some("continuing on the existing runtime")] {
        let total = usage(&[
            report(100, 50, 90, None),
            handover(),
            status(SessionStatus::Rotating, Some("attempting a handover")),
            report(110, 55, 70, None),
            status(SessionStatus::Ready, detail),
            report(120, 60, 40, None),
        ]);

        assert_eq!((total.input_tokens, total.output_tokens), (120, 60));
    }
}

#[test]
fn markers_reset_only_token_maxima_and_context_keeps_latest_stated_values() {
    let total = usage(&[
        report(50, 20, 120, Some(200)),
        marker(1),
        report(2, 1, 0, None),
        report(4, 2, 40, None),
        marker(2),
        report(1, 1, 0, Some(300)),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (55, 23));
    assert_eq!(total.context_used, 40, "zero is not a context reading");
    assert_eq!(
        total.context_window,
        Some(300),
        "absence does not erase a ceiling and a later stated ceiling wins"
    );
}

#[test]
fn an_unrepresentable_cross_runtime_sum_saturates_instead_of_wrapping() {
    let total = usage(&[
        report(u64::MAX, u64::MAX, 1, None),
        marker(1),
        report(1, 1, 1, None),
    ]);

    assert_eq!(total.input_tokens, u64::MAX);
    assert_eq!(total.output_tokens, u64::MAX);
}

#[test]
fn a_legacy_prefix_and_its_first_exact_marker_are_monotonic() {
    let kinds = [
        report(100, 50, 100, None),
        status(SessionStatus::Rotating, Some("legacy handover")),
        report(10, 5, 10, None),
        marker(1),
        report(7, 3, 7, None),
    ];

    let totals = (1..=kinds.len())
        .map(|len| usage(&kinds[..len]))
        .collect::<Vec<_>>();
    assert_eq!(
        totals
            .iter()
            .map(|total| (total.input_tokens, total.output_tokens))
            .collect::<Vec<_>>(),
        [(100, 50), (100, 50), (110, 55), (110, 55), (117, 58)]
    );
    assert!(totals.windows(2).all(|pair| {
        pair[0].input_tokens <= pair[1].input_tokens
            && pair[0].output_tokens <= pair[1].output_tokens
    }));
}

#[test]
fn outgoing_reports_stay_in_the_same_segment_until_the_authority_switch() {
    let preparing = [
        report(100, 50, 90, None),
        handover(),
        status(SessionStatus::Rotating, Some("coordinated handover")),
        report(110, 55, 80, None),
    ];
    let before_switch = usage(&preparing);
    assert_eq!(
        (before_switch.input_tokens, before_switch.output_tokens),
        (110, 55)
    );

    let mut switched = preparing.to_vec();
    switched.extend([marker(1), report(7, 3, 7, None)]);
    let after_switch = usage(&switched);
    assert_eq!(
        (after_switch.input_tokens, after_switch.output_tokens),
        (117, 58)
    );

    let dipped = usage(&[
        report(100, 50, 90, None),
        handover(),
        status(SessionStatus::Rotating, Some("coordinated handover")),
        report(10, 5, 10, None),
        marker(1),
        report(7, 3, 7, None),
    ]);
    assert_eq!(
        (dipped.input_tokens, dipped.output_tokens),
        (107, 53),
        "an outgoing counter dip is not a replacement reset before the switch marker"
    );
}

#[test]
fn compatibility_handover_keeps_late_h1_usage_until_h2_is_durably_named() {
    let total = usage(&[
        incarnation(),
        report(100, 50, 90, None),
        handover(),
        status(SessionStatus::Rotating, Some("compatibility handover")),
        report(110, 55, 80, None),
        incarnation(),
        report(7, 3, 7, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (117, 58));
}

#[test]
fn compatibility_rollback_keeps_h1_in_one_incarnation() {
    let total = usage(&[
        incarnation(),
        report(100, 50, 90, None),
        handover(),
        status(SessionStatus::Rotating, Some("compatibility handover")),
        report(110, 55, 80, None),
        status(
            SessionStatus::Ready,
            Some("continuing on the existing runtime"),
        ),
        report(120, 60, 70, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (120, 60));
}

#[test]
fn compatibility_and_coordinated_markers_compose_without_weakening_generation_ordering() {
    let total = usage(&[
        incarnation(),
        report(100, 50, 90, None),
        marker(4),
        report(10, 5, 10, None),
        marker(4),
        marker(3),
        report(20, 8, 20, None),
        marker(5),
        report(7, 3, 7, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (127, 61));
}

#[test]
fn repeated_rotating_statuses_do_not_split_a_pending_coordinated_handover() {
    let total = usage(&[
        report(100, 50, 90, None),
        handover(),
        status(SessionStatus::Rotating, None),
        report(110, 55, 80, None),
        status(SessionStatus::Rotating, Some("another replica observed it")),
        report(120, 60, 70, None),
        marker(1),
        report(7, 3, 7, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (127, 63));
}

#[test]
fn a_handover_start_permanently_enables_exact_marker_accounting() {
    let total = usage(&[
        report(100, 50, 90, None),
        handover(),
        EventKind::TurnStarted,
        status(
            SessionStatus::Rotating,
            Some("readopted coordinated handover"),
        ),
        report(10, 5, 10, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (100, 50));
}

#[test]
fn an_unrelated_detailed_ready_does_not_merge_a_real_legacy_boundary() {
    let total = usage(&[
        report(100, 50, 90, None),
        status(SessionStatus::Rotating, Some("legacy handover")),
        status(SessionStatus::Ready, Some("an unrelated explanation")),
        report(10, 5, 10, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (110, 55));
}

#[test]
fn two_legacy_rotations_each_close_exactly_one_segment() {
    let total = usage(&[
        report(100, 50, 90, None),
        status(SessionStatus::Rotating, None),
        report(10, 5, 10, None),
        status(SessionStatus::Rotating, None),
        report(5, 2, 5, None),
    ]);

    assert_eq!((total.input_tokens, total.output_tokens), (115, 57));
}

#[test]
fn input_and_output_saturate_independently() {
    let input_saturated = usage(&[
        report(u64::MAX, 5, 1, None),
        status(SessionStatus::Rotating, None),
        report(1, 7, 1, None),
    ]);
    assert_eq!(
        (input_saturated.input_tokens, input_saturated.output_tokens),
        (u64::MAX, 12)
    );

    let output_saturated = usage(&[
        report(5, u64::MAX, 1, None),
        status(SessionStatus::Rotating, None),
        report(7, 1, 1, None),
    ]);
    assert_eq!(
        (
            output_saturated.input_tokens,
            output_saturated.output_tokens
        ),
        (12, u64::MAX)
    );
}
