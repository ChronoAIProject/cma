use super::*;
use layer2_core::sandbox_usage::{Allocation, UsageEventKind, UsageTotals};
use layer2_core::{SandboxId, SubjectId};

fn state() -> SandboxUsage {
    SandboxUsage {
        sandbox_id: SandboxId("sandbox".into()),
        owner: SubjectId("owner".into()),
        name: "sandbox".into(),
        status: "active".into(),
        allocation: Allocation {
            running: true,
            cpu_millis: 1375,
            memory_mib: 1700,
            disk_gib: 8,
        },
        created_at: DAY,
        tracking_started_at: DAY,
        last_observed_at: 32 * DAY,
        source_seq: 31,
        source_revision: 31,
        revision: 31,
        has_started: true,
        complete_lifecycle: true,
        totals: UsageTotals::default(),
    }
}

fn events(state: &SandboxUsage) -> Vec<UsageEvent> {
    (0..31)
        .map(|n| {
            let mut allocation = state.allocation.clone();
            allocation.cpu_millis += n as u32;
            allocation.running = n % 3 != 0;
            UsageEvent {
                sandbox_id: state.sandbox_id.clone(),
                owner: state.owner.clone(),
                revision: n + 1,
                source_seq: n + 1,
                kind: UsageEventKind::Checkpoint,
                started_at: (n + 1) * DAY,
                ended_at: (n + 2) * DAY,
                quantity: allocation.usage(DAY).unwrap(),
                next_allocation: allocation.clone(),
                allocation,
                status: "active".into(),
            }
        })
        .collect()
}

#[test]
fn allocation_cold_range_preserves_daily_values_at_arbitrary_edges_and_corrupt_histories() {
    let state = state();
    for days in [1, 7, 30] {
        let range = Range {
            from: DAY + 123,
            to: (days + 1) * DAY + 123,
        };
        let slices = slices(range);
        for defect in 0..6 {
            let mut history = events(&state);
            match defect {
                1 => {
                    history.remove(1);
                }
                2 => {
                    history.insert(1, history[0].clone());
                }
                3 => {
                    history[0].owner = SubjectId("foreign".into());
                }
                4 => {
                    history[0].ended_at = 5 * DAY;
                    history[0].quantity = history[0].allocation.usage(4 * DAY).unwrap();
                }
                5 => {
                    for event in &mut history {
                        event.revision += 1_u64 << 54;
                    }
                }
                _ => {}
            }
            let mut prepared = Prepared::new();
            let mut budget = OutputBudget::default();
            assert!(prepare_group(
                &mut prepared,
                &mut budget,
                std::slice::from_ref(&state),
                &BTreeMap::from([(state.sandbox_id.to_string(), history.clone())]),
                &slices
            )
            .unwrap());
            assert_eq!(prepared.len(), slices.len());
            for slice in &slices {
                let daily: Vec<_> = history
                    .iter()
                    .filter(|e| e.ended_at > slice.from && e.started_at < slice.to)
                    .cloned()
                    .collect();
                assert_eq!(
                    serde_json::to_value(&prepared[&slice.from][state.sandbox_id.as_str()])
                        .unwrap(),
                    serde_json::to_value(allocation::project(&state, &daily, *slice)).unwrap(),
                    "days={days}, defect={defect}, slice={slice:?}"
                );
            }
        }
    }
}

#[test]
fn allocation_cold_range_foreign_event_cannot_contaminate_nonoverlapping_days() {
    let state = state();
    let mut history = events(&state);
    history[0].owner = SubjectId("foreign".into());
    let mut prepared = Prepared::new();
    assert!(prepare_group(
        &mut prepared,
        &mut OutputBudget::default(),
        std::slice::from_ref(&state),
        &BTreeMap::from([(state.sandbox_id.to_string(), history)]),
        &slices(Range {
            from: DAY,
            to: 4 * DAY
        })
    )
    .unwrap());
    assert!(!prepared[&DAY][state.sandbox_id.as_str()].period.complete);
    assert!(
        prepared[&(2 * DAY)][state.sandbox_id.as_str()]
            .period
            .complete
    );
    assert!(
        prepared[&(3 * DAY)][state.sandbox_id.as_str()]
            .period
            .complete
    );
}

#[test]
fn allocation_cold_range_output_count_and_byte_caps_decline_without_errors() {
    let state = state();
    let range = Range {
        from: DAY,
        to: 2 * DAY,
    };
    let projection = allocation::project(&state, &events(&state)[..1], range);
    assert_eq!(projection.expectations.len(), 1);
    let bytes = mongodb::bson::serialize_to_vec(&cache_update(&state, range, &projection).unwrap())
        .unwrap()
        .len();
    let mut count = OutputBudget {
        expectations: MAX_EXPECTATIONS - 1,
        bytes: 0,
    };
    assert!(count.admit(&state, range, &projection).unwrap());
    assert!(!count.admit(&state, range, &projection).unwrap());
    let mut size = OutputBudget {
        expectations: 0,
        bytes: MAX_PREPARED_BYTES - bytes,
    };
    assert!(size.admit(&state, range, &projection).unwrap());
    assert!(!size.admit(&state, range, &projection).unwrap());
    let mut saturated = OutputBudget {
        expectations: usize::MAX,
        bytes: usize::MAX,
    };
    assert!(!saturated.admit(&state, range, &projection).unwrap());
}

#[test]
fn allocation_cold_range_splits_at_utc_days_without_rounding_edges() {
    let range = Range {
        from: DAY + 123,
        to: 31 * DAY + 123,
    };
    let slices = slices(range);
    assert_eq!(slices.len(), 31);
    assert_eq!(slices[0].from, range.from);
    assert_eq!(slices.last().unwrap().to, range.to);
    assert_eq!(
        slices.iter().map(|r| r.seconds()).sum::<u64>(),
        range.seconds()
    );
    assert!(slices
        .windows(2)
        .all(|w| w[0].to == w[1].from && w[0].to % DAY == 0));
}
