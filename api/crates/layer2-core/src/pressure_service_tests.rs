//! Pressure readings crossing the live-entry and durable-log boundary.

use std::sync::Arc;
use std::time::{Duration, Instant};

use layer3_adapter::ResourceSample;

use crate::authz::Caller;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::pressure::{CpuPressureSample, PressureSample};
use crate::resources::{cpu_pressure_sample, memory_pressure_sample, ResourceReading};
use crate::service::SessionService;
use crate::session::{Session, SessionStatus};
use crate::test_support::{await_settled, service, spec, user, FakeAdapter, FakeStore};

async fn ready(service: &Arc<SessionService>, caller: &Caller) -> Session {
    let created = service
        .create_session(caller, spec())
        .await
        .expect("session created");
    await_settled(service, caller, &created.id).await
}

fn sample(at: Instant, occupancy_permille: u16) -> PressureSample {
    PressureSample {
        occupancy_permille: Some(occupancy_permille),
        used_bytes: u64::from(occupancy_permille),
        limit_bytes: Some(1_000),
        at,
    }
}

async fn feed(
    service: &SessionService,
    id: &SessionId,
    start: Instant,
    occupancy_permille: u16,
    count: usize,
) -> Instant {
    let mut at = start;
    for _ in 0..count {
        service
            .note_resource_pressure(id, Some(sample(at, occupancy_permille)), None)
            .await;
        at += Duration::from_secs(2);
    }
    at
}

fn cpu_sample(at: Instant, occupancy_permille: u16) -> CpuPressureSample {
    CpuPressureSample {
        occupancy_permille: Some(occupancy_permille),
        used_millis: u32::from(occupancy_permille),
        limit_millis: Some(1_000),
        at,
    }
}

async fn feed_cpu(
    service: &SessionService,
    id: &SessionId,
    start: Instant,
    occupancy_permille: u16,
    count: usize,
) -> Instant {
    let mut at = start;
    for _ in 0..count {
        service
            .note_resource_pressure(id, None, Some(cpu_sample(at, occupancy_permille)))
            .await;
        at += Duration::from_secs(2);
    }
    at
}

async fn pressure_events(
    service: &SessionService,
    caller: &Caller,
    id: &SessionId,
) -> Vec<EventKind> {
    service
        .events_since(caller, id, None)
        .await
        .expect("event log is readable")
        .into_iter()
        .filter_map(|event| {
            matches!(
                &event.kind,
                EventKind::MemoryPressureOpened { .. } | EventKind::MemoryPressureClosed { .. }
            )
            .then_some(event.kind)
        })
        .collect()
}

async fn cpu_pressure_events(
    service: &SessionService,
    caller: &Caller,
    id: &SessionId,
) -> Vec<EventKind> {
    service
        .events_since(caller, id, None)
        .await
        .expect("event log is readable")
        .into_iter()
        .filter_map(|event| {
            matches!(
                &event.kind,
                EventKind::CpuPressureOpened { .. } | EventKind::CpuPressureClosed { .. }
            )
            .then_some(event.kind)
        })
        .collect()
}

async fn set_status(service: &SessionService, id: &SessionId, status: SessionStatus) {
    service
        .entries
        .write()
        .await
        .get_mut(id)
        .expect("session has a live entry")
        .session
        .status = status;
}

fn reading(memory_bytes_used: Option<u64>, memory_bytes_limit: Option<u64>) -> ResourceReading {
    ResourceReading {
        cpu_millis_used: None,
        cpu_millis_limit: None,
        memory_bytes_used,
        memory_bytes_limit,
        memory_bytes_peak: None,
        stall_permille: None,
        ceiling_hits: None,
        allocation_failures: None,
        kills: None,
        received_bytes: None,
        sent_bytes: None,
        received_bytes_per_second: None,
        sent_bytes_per_second: None,
        interval_ms: None,
    }
}

fn reported_sample(
    memory_bytes_used: Option<u64>,
    memory_bytes_limit: Option<u64>,
    number: u64,
) -> ResourceSample {
    ResourceSample {
        cpu_millis_used: None,
        cpu_millis_limit: None,
        memory_bytes_used,
        memory_bytes_limit,
        memory_bytes_peak: None,
        stall_permille: None,
        ceiling_hits: None,
        allocation_failures: None,
        kills: None,
        received_bytes: None,
        sent_bytes: None,
        received_bytes_per_second: None,
        sent_bytes_per_second: None,
        interval_ms: Some(number),
    }
}

async fn resource_sender(adapter: &FakeAdapter) -> tokio::sync::mpsc::Sender<ResourceSample> {
    for _ in 0..400 {
        if let Some(sender) = adapter.resource_sink.lock().await.clone() {
            return sender;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("resource watch was not opened");
}

async fn await_last_interval(
    service: &SessionService,
    caller: &Caller,
    id: &SessionId,
    wanted: u64,
) {
    for _ in 0..400 {
        if matches!(
            service.last_reading(caller, id).await,
            Ok(crate::ResourceStatus::Held(ref held))
                if held.reading.interval_ms == Some(wanted)
        ) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("the final reading was not held");
}

#[test]
fn a_reading_without_memory_use_produces_no_pressure_sample() {
    assert!(memory_pressure_sample(&reading(None, Some(1_000)), Instant::now()).is_none());
}

#[test]
fn a_reading_without_a_limit_preserves_that_absence() {
    let at = Instant::now();
    let sample = memory_pressure_sample(&reading(Some(850), None), at).expect("use was reported");
    assert_eq!(sample.used_bytes, 850);
    assert_eq!(sample.limit_bytes, None);
    assert_eq!(sample.occupancy_permille, None);
    assert_eq!(sample.at, at);
}

#[test]
fn occupancy_is_derived_without_overflow_or_rounding_up() {
    let sample = memory_pressure_sample(&reading(Some(u64::MAX), Some(u64::MAX)), Instant::now())
        .expect("use was reported");
    assert_eq!(sample.occupancy_permille, Some(1_000));

    let sample = memory_pressure_sample(&reading(Some(799), Some(1_000)), Instant::now())
        .expect("use was reported");
    assert_eq!(sample.occupancy_permille, Some(799));
}

#[test]
fn cpu_pressure_requires_both_a_rate_and_a_truthful_limit() {
    let at = Instant::now();
    let mut reading = reading(None, None);
    reading.cpu_millis_limit = Some(1_000);
    assert!(cpu_pressure_sample(&reading, at).is_none());

    reading.cpu_millis_used = Some(850);
    reading.cpu_millis_limit = None;
    let sample = cpu_pressure_sample(&reading, at).expect("the CPU rate was reported");
    assert_eq!(sample.used_millis, 850);
    assert_eq!(sample.limit_millis, None);
    assert_eq!(sample.occupancy_permille, None);

    reading.cpu_millis_limit = Some(1_000);
    let sample = cpu_pressure_sample(&reading, at).expect("both figures were reported");
    assert_eq!(sample.limit_millis, Some(1_000));
    assert_eq!(sample.occupancy_permille, Some(850));
}

#[tokio::test]
async fn the_resource_fanout_drives_pressure_but_absent_memory_is_silence() {
    let store = Arc::new(FakeStore::default());
    let mut fake = FakeAdapter::default();
    fake.capabilities.resources = true;
    let adapter = Arc::new(fake);
    let service = service(store, Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&service, &alice).await;
    let sender = resource_sender(&adapter).await;

    for number in 1..=5 {
        sender
            .send(reported_sample(Some(850), Some(1_000), number))
            .await
            .expect("watch remains open");
    }
    for _ in 0..400 {
        if pressure_events(&service, &alice, &session.id).await.len() == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(matches!(
        pressure_events(&service, &alice, &session.id)
            .await
            .as_slice(),
        [EventKind::MemoryPressureOpened { .. }]
    ));

    for number in 6..=105 {
        sender
            .send(reported_sample(None, Some(1_000), number))
            .await
            .expect("watch remains open");
    }
    await_last_interval(&service, &alice, &session.id, 105).await;
    assert_eq!(
        pressure_events(&service, &alice, &session.id).await.len(),
        1
    );
}

#[tokio::test]
async fn the_resource_fanout_drives_cpu_pressure_but_absent_cpu_is_silence() {
    let store = Arc::new(FakeStore::default());
    let mut fake = FakeAdapter::default();
    fake.capabilities.resources = true;
    let adapter = Arc::new(fake);
    let service = service(store, Arc::clone(&adapter));
    let alice = user("alice");
    let session = ready(&service, &alice).await;
    let sender = resource_sender(&adapter).await;

    for number in 1..=5 {
        let mut sample = reported_sample(None, None, number);
        sample.cpu_millis_used = Some(850);
        sample.cpu_millis_limit = Some(1_000);
        sender.send(sample).await.expect("watch remains open");
    }
    for _ in 0..400 {
        if cpu_pressure_events(&service, &alice, &session.id)
            .await
            .len()
            == 1
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(matches!(
        cpu_pressure_events(&service, &alice, &session.id)
            .await
            .as_slice(),
        [EventKind::CpuPressureOpened { .. }]
    ));

    for number in 6..=105 {
        sender
            .send(reported_sample(None, None, number))
            .await
            .expect("watch remains open");
    }
    await_last_interval(&service, &alice, &session.id, 105).await;
    assert_eq!(
        cpu_pressure_events(&service, &alice, &session.id)
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn provisioning_and_rotating_sessions_suppress_pressure() {
    for status in [SessionStatus::Provisioning, SessionStatus::Rotating] {
        let store = Arc::new(FakeStore::default());
        let service = service(store, Arc::new(FakeAdapter::default()));
        let alice = user("alice");
        let session = ready(&service, &alice).await;
        set_status(&service, &session.id, status).await;

        feed(&service, &session.id, Instant::now(), 950, 100).await;
        feed_cpu(&service, &session.id, Instant::now(), 950, 100).await;
        assert!(
            pressure_events(&service, &alice, &session.id)
                .await
                .is_empty(),
            "{status:?} produced a pressure edge"
        );
        assert!(
            cpu_pressure_events(&service, &alice, &session.id)
                .await
                .is_empty(),
            "{status:?} produced a CPU-pressure edge"
        );
    }
}

#[tokio::test]
async fn leaving_interactive_status_closes_once_and_evaluates_nothing_further() {
    let store = Arc::new(FakeStore::default());
    let service = service(store, Arc::new(FakeAdapter::default()));
    let alice = user("alice");
    let session = ready(&service, &alice).await;
    set_status(&service, &session.id, SessionStatus::Running).await;
    let next = feed(&service, &session.id, Instant::now(), 850, 5).await;
    assert!(matches!(
        pressure_events(&service, &alice, &session.id)
            .await
            .as_slice(),
        [EventKind::MemoryPressureOpened { .. }]
    ));

    set_status(&service, &session.id, SessionStatus::Rotating).await;
    feed(&service, &session.id, next, 950, 100).await;
    assert!(matches!(
        pressure_events(&service, &alice, &session.id)
            .await
            .as_slice(),
        [
            EventKind::MemoryPressureOpened { .. },
            EventKind::MemoryPressureClosed { .. }
        ]
    ));
}

#[tokio::test]
async fn leaving_interactive_status_closes_cpu_pressure_once_and_evaluates_nothing_further() {
    let store = Arc::new(FakeStore::default());
    let service = service(store, Arc::new(FakeAdapter::default()));
    let alice = user("alice");
    let session = ready(&service, &alice).await;
    set_status(&service, &session.id, SessionStatus::Running).await;
    let next = feed_cpu(&service, &session.id, Instant::now(), 850, 5).await;
    assert!(matches!(
        cpu_pressure_events(&service, &alice, &session.id)
            .await
            .as_slice(),
        [EventKind::CpuPressureOpened { .. }]
    ));

    set_status(&service, &session.id, SessionStatus::Rotating).await;
    feed_cpu(&service, &session.id, next, 950, 100).await;
    assert!(matches!(
        cpu_pressure_events(&service, &alice, &session.id)
            .await
            .as_slice(),
        [
            EventKind::CpuPressureOpened { .. },
            EventKind::CpuPressureClosed { .. }
        ]
    ));
}
