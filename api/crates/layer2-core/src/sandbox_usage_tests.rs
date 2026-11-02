use super::lifecycle_test_support::sandbox;
use super::SandboxStatus;
use crate::sandbox_usage::*;

fn at(seconds: u64) -> String {
    crate::rfc3339_at(
        std::time::UNIX_EPOCH
            + std::time::Duration::from_secs(timestamp("2026-09-05T00:00:00Z").unwrap() + seconds),
    )
}

#[test]
fn sandbox_usage_start_pause_resume_terminate_counts_runtime_intervals_once() {
    let mut value = sandbox();
    let runtime = value.runtime.take();
    value.status = SandboxStatus::Provisioning;
    let (created, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    value.runtime = runtime.clone();
    value.status = SandboxStatus::Installing;
    let (started, event) = observe(Some(&created), &value, 2, &at(10), false)
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, UsageEventKind::Started);
    assert_eq!(event.quantity.uptime_seconds, 0);
    value.status = SandboxStatus::Pausing;
    let (pausing, _) = observe(Some(&started), &value, 3, &at(50), false)
        .unwrap()
        .unwrap();
    value.runtime = None;
    value.status = SandboxStatus::Paused;
    let (paused, event) = observe(Some(&pausing), &value, 4, &at(70), false)
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, UsageEventKind::Paused);
    assert_eq!(paused.totals.uptime_seconds, 60);
    assert_eq!(
        paused.totals_at(timestamp(&at(100)).unwrap()).unwrap(),
        paused.totals
    );
    value.status = SandboxStatus::Reviving;
    value.runtime = runtime;
    let (resumed, event) = observe(Some(&paused), &value, 5, &at(100), false)
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, UsageEventKind::Resumed);
    assert_eq!(event.quantity.uptime_seconds, 0);
    value.runtime = None;
    value.status = SandboxStatus::Retired;
    let (retired, event) = observe(Some(&resumed), &value, 6, &at(130), false)
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, UsageEventKind::Terminated);
    assert_eq!(
        retired.totals,
        UsageTotals {
            uptime_seconds: 90,
            cpu_millis_seconds: 90_000,
            memory_mib_seconds: 184_320,
            disk_gib_seconds: 0,
        }
    );
    assert!(observe(Some(&retired), &value, 6, &at(130), false)
        .unwrap()
        .is_none());
    assert!(retired.complete_lifecycle);
}

#[test]
fn sandbox_usage_retained_storage_accrues_while_paused_and_clips_reporting_periods() {
    let event = UsageEvent {
        sandbox_id: crate::SandboxId("sandbox-storage".into()),
        owner: crate::SubjectId("owner".into()),
        revision: 2,
        source_seq: 3,
        kind: UsageEventKind::Checkpoint,
        started_at: 100,
        ended_at: 200,
        allocation: Allocation {
            disk_gib: 20,
            ..Default::default()
        },
        next_allocation: Allocation::default(),
        status: "paused".into(),
        quantity: UsageTotals::default(),
    };
    assert_eq!(
        event.within(125, 175).unwrap(),
        UsageTotals {
            disk_gib_seconds: 1_000,
            ..Default::default()
        }
    );
    assert_eq!(event.within(200, 300).unwrap(), UsageTotals::default());
    assert_eq!(event.within(1, 100).unwrap(), UsageTotals::default());
    assert!(event.within(200, 100).is_err());
}

#[test]
fn sandbox_usage_checkpoints_split_intervals_without_rounding_loss_or_double_charging() {
    let mut value = sandbox();
    value.profile.content.cpu_millis = 125;
    let (first, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    let (second, event) = observe(Some(&first), &value, 1, &at(7), true)
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, UsageEventKind::Checkpoint);
    assert_eq!(event.quantity.cpu_millis_seconds, 875);
    assert!(observe(Some(&second), &value, 1, &at(7), true)
        .unwrap()
        .is_none());
    let (third, next) = observe(Some(&second), &value, 1, &at(11), true)
        .unwrap()
        .unwrap();
    assert_ne!(event.transaction_id(), next.transaction_id());
    assert_eq!(third.totals.cpu_millis_seconds, 1_375);
    assert_eq!(
        third
            .totals_at(timestamp(&at(15)).unwrap())
            .unwrap()
            .cpu_millis_seconds,
        1_875
    );
}

#[test]
fn sandbox_usage_baseline_does_not_invent_unobserved_history() {
    let value = sandbox();
    let (baseline, event) = observe(None, &value, 42, &at(600), false).unwrap().unwrap();
    assert!(!baseline.complete_lifecycle);
    assert_eq!(event.kind, UsageEventKind::Baseline);
    assert_eq!(baseline.totals.uptime_seconds, 0);
    assert_eq!(
        baseline
            .totals_at(timestamp(&at(630)).unwrap())
            .unwrap()
            .uptime_seconds,
        30
    );
}

#[test]
fn sandbox_usage_rejects_bad_time_ownership_sequence_and_overflow() {
    let mut value = sandbox();
    assert!(observe(None, &value, 1, "invalid", false).is_err());
    assert!(timestamp("1970-01-01T00:00:00Z").is_err());
    let (first, _) = observe(None, &value, 5, &at(10), false).unwrap().unwrap();
    assert_eq!(
        observe(Some(&first), &value, 4, &at(11), false),
        Err(UsageError::Conflict)
    );
    value.owner = crate::SubjectId("other-owner".into());
    assert_eq!(
        observe(Some(&first), &value, 6, &at(11), false),
        Err(UsageError::Conflict)
    );
    assert!(Allocation {
        cpu_millis: 2,
        ..Default::default()
    }
    .usage(u64::MAX)
    .is_err());
    assert!(first.totals_at(timestamp(&at(9)).unwrap()).is_err());
}

#[test]
fn sandbox_usage_clock_skew_does_not_subtract_or_duplicate_time() {
    let mut value = sandbox();
    let (first, _) = observe(None, &value, 1, &at(0), false).unwrap().unwrap();
    let (second, _) = observe(Some(&first), &value, 2, &at(10), true)
        .unwrap()
        .unwrap();
    value.runtime = None;
    value.status = SandboxStatus::Paused;
    let (paused, event) = observe(Some(&second), &value, 3, &at(9), false)
        .unwrap()
        .unwrap();
    assert_eq!(event.started_at, event.ended_at);
    assert_eq!(paused.totals.uptime_seconds, 10);
}
