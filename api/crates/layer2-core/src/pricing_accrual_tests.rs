use super::*;

use crate::ids::SessionId;
use crate::session::ProvisioningStage;

const CREATED: &str = "2026-08-23T01:00:00Z";
const NOW: u64 = 2_000_000_000;

fn pricing() -> SessionPricing {
    SessionPricing {
        rate_card: RateCardId("rate_test".into()),
        rate: Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        },
        cpu_millis: 1_000,
        memory_mib: 1_024,
        micros_per_hour: 45_000,
        quoted_at: CREATED.into(),
    }
}

fn event(seq: u64, at: &str, kind: EventKind) -> Event {
    Event {
        session_id: SessionId("sesn_priced".into()),
        seq,
        kind,
        appended_at: at.into(),
    }
}

fn status(seq: u64, at: &str, status: SessionStatus, reason: Option<StopReason>) -> Event {
    event(
        seq,
        at,
        EventKind::StatusChanged {
            status,
            stop_reason: reason,
            detail: None,
            capability: None,
        },
    )
}

#[test]
fn a_session_that_never_became_ready_accrues_nothing() {
    let events = vec![
        status(1, CREATED, SessionStatus::Provisioning, None),
        status(
            2,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::Error),
        ),
    ];

    assert_eq!(
        accrue(&events, &pricing(), CREATED, NOW),
        Accrual {
            billable_seconds: 0,
            micros: 0,
            open: false,
            since: None,
            unbounded_intervals: 0,
        }
    );
}

#[test]
fn the_interval_opens_at_ready_and_not_at_provisioning() {
    let events = vec![
        status(1, CREATED, SessionStatus::Provisioning, None),
        status(2, "2026-08-23T01:10:00Z", SessionStatus::Ready, None),
        status(
            3,
            "2026-08-23T01:20:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    let total = accrue(&events, &pricing(), CREATED, NOW);
    assert_eq!(total.billable_seconds, 600);
    assert_eq!(total.micros, 7_500);
}

#[test]
fn an_hour_at_forty_five_thousand_micros_accrues_forty_five_thousand() {
    let events = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    let total = accrue(&events, &pricing(), CREATED, NOW);
    assert_eq!((total.billable_seconds, total.micros), (3_600, 45_000));
}

#[test]
fn a_rotation_is_one_interval_and_is_not_charged_twice() {
    let events = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(2, "2026-08-23T01:15:00Z", SessionStatus::Rotating, None),
        status(3, "2026-08-23T01:30:00Z", SessionStatus::Ready, None),
        status(
            4,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    assert_eq!(
        accrue(&events, &pricing(), CREATED, NOW).billable_seconds,
        3_600
    );
}

#[test]
fn an_approval_held_open_is_still_accruing() {
    let events = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T01:15:00Z",
            SessionStatus::Idle,
            Some(StopReason::RequiresAction),
        ),
        status(
            3,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    assert_eq!(
        accrue(&events, &pricing(), CREATED, NOW).billable_seconds,
        3_600
    );
}

#[test]
fn a_stopped_session_accrues_nothing_while_stopped() {
    let events = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T01:10:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
        status(3, "2026-08-23T02:00:00Z", SessionStatus::Ready, None),
        status(
            4,
            "2026-08-23T02:20:00Z",
            SessionStatus::Terminated,
            Some(StopReason::UserTerminated),
        ),
    ];

    assert_eq!(
        accrue(&events, &pricing(), CREATED, NOW).billable_seconds,
        1_800
    );
}

#[test]
fn an_open_interval_reports_that_it_is_accruing_and_when_it_started() {
    let now = seconds_from_rfc3339("2026-08-23T02:00:00Z").expect("test time");
    let events = vec![status(1, CREATED, SessionStatus::Ready, None)];

    let total = accrue(&events, &pricing(), CREATED, now);

    assert!(total.open);
    assert_eq!(total.since.as_deref(), Some(CREATED));
    assert_eq!((total.billable_seconds, total.micros), (3_600, 45_000));
}

#[test]
fn two_folds_of_one_log_agree_exactly() {
    let events = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T02:00:00Z",
            SessionStatus::Terminated,
            Some(StopReason::UserTerminated),
        ),
    ];

    assert_eq!(
        accrue(&events, &pricing(), CREATED, NOW),
        accrue(&events, &pricing(), CREATED, NOW)
    );
}

#[test]
fn an_event_with_no_appended_at_is_skipped_and_counted() {
    let events = vec![
        status(1, "", SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    let total = accrue(&events, &pricing(), CREATED, NOW);
    assert_eq!((total.billable_seconds, total.micros), (0, 0));
    assert_eq!(total.unbounded_intervals, 1);
}

#[test]
fn an_unparseable_interval_end_is_skipped_and_counted() {
    let events = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(
            2,
            "not-a-time",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    let total = accrue(&events, &pricing(), CREATED, NOW);
    assert_eq!(total.billable_seconds, 0);
    assert_eq!(total.unbounded_intervals, 1);
}

#[test]
fn a_timestamp_before_the_session_existed_is_refused() {
    let events = vec![
        status(1, "2026-08-23T00:59:59Z", SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    let total = accrue(&events, &pricing(), CREATED, NOW);
    assert_eq!(total.billable_seconds, 0);
    assert_eq!(total.unbounded_intervals, 1);
}

#[test]
fn the_epoch_is_refused_as_an_interval_bound() {
    let events = vec![
        status(1, "1970-01-01T00:00:00Z", SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    assert_eq!(
        accrue(&events, &pricing(), CREATED, NOW).unbounded_intervals,
        1
    );
}

#[test]
fn an_interval_ending_after_now_is_skipped_and_counted() {
    let now = seconds_from_rfc3339("2026-08-23T01:30:00Z").expect("test time");
    let events = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    let total = accrue(&events, &pricing(), CREATED, now);
    assert_eq!(total.billable_seconds, 0);
    assert_eq!(total.unbounded_intervals, 1);
}

#[test]
fn a_reversed_pair_contributes_zero_rather_than_underflowing() {
    let events = vec![
        status(1, "2026-08-23T02:00:00Z", SessionStatus::Ready, None),
        status(
            2,
            "2026-08-23T01:30:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];

    let total = accrue(&events, &pricing(), CREATED, NOW);
    assert_eq!((total.billable_seconds, total.micros), (0, 0));
    assert_eq!(total.unbounded_intervals, 0);
}

#[test]
fn only_status_changed_is_read() {
    let statuses = vec![
        status(1, CREATED, SessionStatus::Ready, None),
        status(
            5,
            "2026-08-23T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];
    let mut crowded = statuses.clone();
    crowded.insert(1, event(2, "", EventKind::SessionCreated));
    crowded.insert(
        2,
        event(
            3,
            "",
            EventKind::Progress {
                stage: ProvisioningStage::Ready,
            },
        ),
    );
    crowded.insert(
        3,
        event(
            4,
            "",
            EventKind::UsageReported {
                input_tokens: 9,
                output_tokens: 8,
                context_used: 7,
                context_window: Some(6),
            },
        ),
    );

    assert_eq!(
        accrue(&crowded, &pricing(), CREATED, NOW),
        accrue(&statuses, &pricing(), CREATED, NOW)
    );
}
