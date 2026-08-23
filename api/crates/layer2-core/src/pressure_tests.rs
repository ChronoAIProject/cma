//! Memory-pressure policy and fold behavior.

use crate::{InvalidPressurePolicy, MemoryPressurePolicy};

#[test]
fn the_shipped_policy_is_valid() {
    let shipped = MemoryPressurePolicy::default();
    let rebuilt = MemoryPressurePolicy::new(
        shipped.enter_permille(),
        shipped.exit_permille(),
        shipped.enter_samples(),
        shipped.exit_samples(),
        shipped.settle_seconds(),
    );
    assert_eq!(rebuilt, Ok(shipped));
}

#[test]
fn an_entry_threshold_outside_permille_is_refused() {
    for (got, expected) in [
        (0, InvalidPressurePolicy::ThresholdOutOfRange { got: 0 }),
        (
            1001,
            InvalidPressurePolicy::ThresholdOutOfRange { got: 1001 },
        ),
    ] {
        assert_eq!(
            MemoryPressurePolicy::new(got, 700, 5, 15, 120),
            Err(expected)
        );
    }
}

#[test]
fn a_twenty_permille_band_is_refused() {
    assert_eq!(
        MemoryPressurePolicy::new(800, 780, 5, 15, 120),
        Err(InvalidPressurePolicy::BandTooNarrow {
            enter: 800,
            exit: 780,
            width: 20,
            min: 50,
        })
    );
}

#[test]
fn one_entry_sample_is_a_crossing_and_is_refused() {
    assert_eq!(
        MemoryPressurePolicy::new(800, 700, 1, 15, 120),
        Err(InvalidPressurePolicy::EntryIsACrossing { got: 1 })
    );
}

#[test]
fn leaving_in_fewer_samples_than_entering_is_refused() {
    assert_eq!(
        MemoryPressurePolicy::new(800, 700, 5, 4, 120),
        Err(InvalidPressurePolicy::LeavingIsEasierThanEntering { enter: 5, exit: 4 })
    );
}

#[test]
fn a_fifty_nine_second_settle_floor_is_refused() {
    assert_eq!(
        MemoryPressurePolicy::new(800, 700, 5, 15, 59),
        Err(InvalidPressurePolicy::SettleTooShort { got: 59, min: 60 })
    );
}

#[test]
fn deserialization_cannot_bypass_policy_validation() {
    let result = serde_json::from_value::<MemoryPressurePolicy>(serde_json::json!({
        "enter_permille": 800,
        "exit_permille": 780,
        "enter_samples": 5,
        "exit_samples": 15,
        "settle_seconds": 120
    }));
    let error = result.expect_err("a narrow stored band is invalid");
    assert!(error.to_string().contains("is 20"), "got {error}");
}

#[test]
fn a_valid_policy_round_trips_through_its_wire_form() {
    let policy = MemoryPressurePolicy::new(850, 650, 6, 18, 180).expect("valid policy");
    let value = serde_json::to_value(policy).expect("serializes");
    assert_eq!(value["enter_permille"], 850);
    assert_eq!(
        serde_json::from_value::<MemoryPressurePolicy>(value).expect("deserializes"),
        policy
    );
}
