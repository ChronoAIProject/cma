use super::*;
use crate::sandbox_usage::{Allocation, UsageError, UsageEvent, UsageEventKind, UsageTotals};
use crate::{SandboxId, SubjectId};

fn source(started_at: u64, ended_at: u64) -> UsageEvent {
    let allocation = Allocation {
        running: true,
        cpu_millis: 1_337,
        memory_mib: 2_049,
        disk_gib: 13,
    };
    UsageEvent {
        sandbox_id: SandboxId("sbx_11111111111111111111111111111111".into()),
        owner: SubjectId::new("unfunded-fixture"),
        revision: 2,
        source_seq: 1,
        kind: UsageEventKind::Checkpoint,
        started_at,
        ended_at,
        quantity: allocation.usage(ended_at - started_at).unwrap(),
        next_allocation: allocation.clone(),
        allocation,
        status: "active".into(),
    }
}

#[test]
fn minute_splits_conserve_every_quantity_across_restarts_and_month_boundary() {
    for (start, end) in [
        (59, 121),
        (1, 1),
        (1_798_761_599, 1_798_761_661),
        (7, 500_003),
    ] {
        let event = source(start, end);
        let mut cursor = start;
        let mut total = UsageTotals::default();
        let mut parts = Vec::new();
        loop {
            let batch = project(&event, cursor).unwrap();
            assert_eq!(batch, project(&event.clone(), cursor).unwrap());
            assert!(batch.facts.len() <= MAX_FACTS);
            for fact in &batch.facts {
                assert_eq!(fact.bucket_start % 60, 0);
                assert!(fact.started_at >= fact.bucket_start);
                assert!(fact.ended_at <= fact.bucket_start + 60);
                assert!(fact.ended_at > fact.started_at);
                total = total.checked_add(&fact.quantity).unwrap();
                parts.push(fact.part);
            }
            cursor = batch.next_second;
            if batch.complete {
                break;
            }
        }
        assert_eq!(cursor, end);
        assert_eq!(total, event.quantity);
        assert_eq!(parts, (0..parts.len() as u64).collect::<Vec<_>>());
    }
}

#[test]
fn retained_home_projects_without_terminal_compute() {
    let mut event = source(59, 121);
    event.allocation.running = false;
    event.allocation.cpu_millis = 0;
    event.allocation.memory_mib = 0;
    event.quantity = event.allocation.usage(62).unwrap();
    for fact in project(&event, 59).unwrap().facts {
        assert_eq!(fact.quantity.cpu_millis_seconds, 0);
        assert_eq!(fact.quantity.memory_mib_seconds, 0);
        assert_eq!(fact.quantity.uptime_seconds, 0);
        assert_eq!(
            fact.quantity.disk_gib_seconds,
            fact.retained_home_seconds * 13
        );
    }
}

#[test]
fn invalid_progress_totals_allocation_and_timestamp_overflow_refuse() {
    let event = source(59, 121);
    for cursor in [0, 58, 61, 119, 122] {
        assert_eq!(project(&event, cursor), Err(UsageError::InvalidTime));
    }
    let mut bad = event.clone();
    bad.quantity.cpu_millis_seconds += 1;
    assert_eq!(project(&bad, 59), Err(UsageError::Conflict));
    bad = event.clone();
    bad.allocation.running = false;
    bad.quantity = bad.allocation.usage(62).unwrap();
    assert_eq!(project(&bad, 59), Err(UsageError::Conflict));
    bad = event;
    bad.ended_at = u64::MAX;
    assert_eq!(project(&bad, 59), Err(UsageError::InvalidTime));
}

#[test]
fn integer_projection_keeps_quantities_above_json_double_precision() {
    let mut event = source(1, 100_000_001);
    event.allocation.cpu_millis = u32::MAX;
    event.quantity = event.allocation.usage(100_000_000).unwrap();
    assert!(event.quantity.cpu_millis_seconds > 1 << 53);
    let batch = project(&event, 1).unwrap();
    assert_eq!(
        batch.facts[0].quantity.cpu_millis_seconds,
        u64::from(u32::MAX) * 59
    );
    assert!(!batch.complete);
}
