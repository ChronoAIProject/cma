//! Memory-pressure policy and fold behavior.

use std::time::{Duration, Instant};

use crate::{
    CpuPressure, CpuPressurePolicy, CpuPressureSample, CpuPressureTransition,
    InvalidPressurePolicy, MemoryPressure, MemoryPressurePolicy, PressureSample,
    PressureTransition,
};

struct Driver {
    machine: MemoryPressure,
    policy: MemoryPressurePolicy,
    next_at: Instant,
    transitions: Vec<PressureTransition>,
}

struct CpuDriver {
    machine: CpuPressure,
    policy: CpuPressurePolicy,
    next_at: Instant,
    transitions: Vec<CpuPressureTransition>,
}

impl CpuDriver {
    fn new() -> Self {
        Self {
            machine: CpuPressure::default(),
            policy: CpuPressurePolicy::default(),
            next_at: Instant::now(),
            transitions: Vec::new(),
        }
    }

    fn feed(&mut self, occupancy_permille: Option<u16>) -> Option<CpuPressureTransition> {
        let at = self.next_at;
        self.next_at += Duration::from_secs(2);
        let transition = self.machine.evaluate(
            &CpuPressureSample {
                occupancy_permille,
                used_millis: u32::from(occupancy_permille.unwrap_or(0)),
                limit_millis: occupancy_permille.map(|_| 1_000),
                at,
            },
            &self.policy,
        );
        if let Some(edge) = transition.clone() {
            self.transitions.push(edge);
        }
        transition
    }

    fn feed_many(&mut self, occupancy_permille: Option<u16>, count: usize) {
        for _ in 0..count {
            self.feed(occupancy_permille);
        }
    }
}

impl Driver {
    fn new() -> Self {
        Self {
            machine: MemoryPressure::default(),
            policy: MemoryPressurePolicy::default(),
            next_at: Instant::now(),
            transitions: Vec::new(),
        }
    }

    fn feed(&mut self, occupancy_permille: Option<u16>) -> Option<PressureTransition> {
        let at = self.next_at;
        self.next_at += Duration::from_secs(2);
        let transition = self.machine.evaluate(
            &PressureSample {
                occupancy_permille,
                used_bytes: u64::from(occupancy_permille.unwrap_or(0)),
                limit_bytes: occupancy_permille.map(|_| 1_000),
                at,
            },
            &self.policy,
        );
        if let Some(edge) = transition.clone() {
            self.transitions.push(edge);
        }
        transition
    }

    fn feed_many(&mut self, occupancy_permille: Option<u16>, count: usize) {
        for _ in 0..count {
            self.feed(occupancy_permille);
        }
    }

    /// Fifteen readings establish exit, then sixty more spend the floor.
    fn close_tail(&mut self, occupancy_permille: u16) {
        self.feed_many(Some(occupancy_permille), 75);
    }
}

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

#[test]
fn a_level_held_above_the_threshold_speaks_twice_not_twenty_times() {
    let mut driver = Driver::new();
    driver.feed_many(Some(850), 20);
    driver.close_tail(100);
    assert_eq!(driver.transitions.len(), 2);
    assert!(matches!(
        driver.transitions.as_slice(),
        [
            PressureTransition::Opened { .. },
            PressureTransition::Closed { .. }
        ]
    ));
}

#[test]
fn a_build_spike_never_opens_an_episode() {
    let mut driver = Driver::new();
    driver.feed_many(Some(200), 5);
    driver.feed_many(Some(900), 3);
    driver.feed_many(Some(200), 20);
    assert!(driver.transitions.is_empty());
}

#[test]
fn chatter_across_the_entry_line_never_opens_an_episode() {
    let mut driver = Driver::new();
    for index in 0..40 {
        driver.feed(Some(if index % 2 == 0 { 790 } else { 810 }));
    }
    assert!(driver.transitions.is_empty());
}

#[test]
fn a_dip_inside_the_band_does_not_close_an_episode() {
    let mut driver = Driver::new();
    driver.feed_many(Some(850), 10);
    driver.feed_many(Some(750), 10);
    driver.feed_many(Some(850), 10);
    assert_eq!(driver.transitions.len(), 1);
    driver.close_tail(100);
    assert_eq!(driver.transitions.len(), 2);
}

#[test]
fn a_slow_oscillation_across_the_whole_band_is_one_episode_not_fifty() {
    let mut driver = Driver::new();
    for _ in 0..5 {
        driver.feed_many(Some(850), 10);
        driver.feed_many(Some(650), 10);
    }
    assert_eq!(driver.transitions.len(), 1);
    driver.close_tail(100);
    assert_eq!(driver.transitions.len(), 2);
}

#[test]
fn a_return_inside_the_settle_floor_cancels_the_close_and_writes_nothing() {
    let mut driver = Driver::new();
    driver.feed_many(Some(850), 5);
    driver.feed_many(Some(650), 15);
    driver.feed_many(Some(850), 5);
    driver.feed_many(Some(650), 60);
    assert_eq!(driver.transitions.len(), 1, "the first close was cancelled");
    driver.feed_many(Some(650), 15);
    assert_eq!(driver.transitions.len(), 2);
}

#[test]
fn entering_takes_five_samples_and_leaving_takes_fifteen() {
    let mut driver = Driver::new();
    driver.feed_many(Some(850), 4);
    assert!(driver.transitions.is_empty());
    assert!(matches!(
        driver.feed(Some(850)),
        Some(PressureTransition::Opened { .. })
    ));

    driver.feed_many(Some(650), 14);
    assert_eq!(driver.transitions.len(), 1);
    assert_eq!(
        driver.feed(Some(650)),
        None,
        "the fifteenth starts the floor"
    );
    driver.feed_many(Some(650), 59);
    assert_eq!(driver.transitions.len(), 1);
    assert!(matches!(
        driver.feed(Some(650)),
        Some(PressureTransition::Closed { .. })
    ));
}

#[test]
fn a_reading_with_no_ceiling_emits_nothing_and_is_not_treated_as_full() {
    let mut driver = Driver::new();
    driver.feed_many(None, 100);
    driver.feed_many(Some(850), 4);
    assert!(driver.transitions.is_empty());
    assert!(matches!(
        driver.feed(Some(850)),
        Some(PressureTransition::Opened { .. })
    ));
}

#[test]
fn the_peak_is_the_highest_of_the_episode_not_the_last() {
    let mut driver = Driver::new();
    driver.feed_many(Some(850), 4);
    driver.feed(Some(970));
    driver.feed(Some(820));
    driver.close_tail(100);
    assert!(matches!(
        driver.transitions.last(),
        Some(PressureTransition::Closed {
            peak_bytes: 970,
            peak_permille: 970,
            ..
        })
    ));
}

#[test]
fn abandoning_an_open_episode_closes_it_without_the_floor() {
    let mut driver = Driver::new();
    driver.feed_many(Some(850), 5);
    let closed = driver
        .machine
        .abandon(driver.next_at + Duration::from_secs(1));
    assert!(matches!(closed, Some(PressureTransition::Closed { .. })));
}

#[test]
fn abandoning_a_quiet_machine_says_nothing() {
    assert_eq!(MemoryPressure::default().abandon(Instant::now()), None);
}

#[test]
fn an_adversarial_hour_cannot_exceed_forty_six_events() {
    let mut driver = Driver::new();
    // One cycle is 5 entry readings + 15 exit readings + 60 more readings for
    // the floor: 160 seconds. That permits 23 opens and 22 closes in 3,600
    // seconds, or 45 edges against the 1,800 readings a level signal would emit.
    for index in 0..1_800 {
        driver.feed(Some(if index % 80 < 5 { 850 } else { 650 }));
    }
    assert!(driver.transitions.len() <= 46, "{:?}", driver.transitions);
    assert!(driver.transitions.len() >= 40, "{:?}", driver.transitions);
}

#[test]
fn cpu_pressure_uses_the_same_sustained_hysteretic_contract() {
    let mut driver = CpuDriver::new();
    driver.feed_many(Some(850), 4);
    assert!(driver.transitions.is_empty());
    assert!(matches!(
        driver.feed(Some(850)),
        Some(CpuPressureTransition::Opened {
            used_millis: 850,
            limit_millis: 1_000,
            occupancy_permille: 850,
        })
    ));
    driver.feed_many(Some(850), 20);
    assert_eq!(driver.transitions.len(), 1, "a held level is one opening");
    driver.feed_many(Some(650), 75);
    assert!(matches!(
        driver.transitions.as_slice(),
        [
            CpuPressureTransition::Opened { .. },
            CpuPressureTransition::Closed { .. }
        ]
    ));
}

#[test]
fn cpu_pressure_without_a_truthful_denominator_is_silent() {
    let mut driver = CpuDriver::new();
    driver.feed_many(None, 100);
    driver.feed_many(Some(850), 4);
    assert!(driver.transitions.is_empty());
    assert!(matches!(
        driver.feed(Some(850)),
        Some(CpuPressureTransition::Opened { .. })
    ));
}

#[test]
fn an_adversarial_cpu_hour_cannot_exceed_forty_six_events() {
    let mut driver = CpuDriver::new();
    for index in 0..1_800 {
        driver.feed(Some(if index % 80 < 5 { 850 } else { 650 }));
    }
    assert!(driver.transitions.len() <= 46, "{:?}", driver.transitions);
    assert!(driver.transitions.len() >= 40, "{:?}", driver.transitions);
}

#[test]
fn a_healthy_hour_is_silent() {
    let mut driver = Driver::new();
    for index in 0..1_800 {
        let occupancy = if index % 100 < 3 {
            900
        } else {
            300 + (index % 351) as u16
        };
        driver.feed(Some(occupancy));
    }
    assert!(driver.transitions.is_empty());
}

#[test]
fn the_pressure_module_contains_only_core_vocabulary() {
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/pressure.rs"))
        .expect("pressure source is readable");
    for foreign in [
        "cgroup",
        "memory.current",
        "memory.max",
        "memory.pressure",
        "psi",
        "oom",
        "Gi",
        "cpu.max",
        concat!("ag", "ui"),
        "pty",
        concat!("co", "dex"),
        concat!("claude", "_code"),
        "dsh",
    ] {
        assert!(!source.contains(foreign), "pressure.rs contains {foreign}");
    }
}
