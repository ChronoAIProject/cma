use super::{fanout_support::*, routing_fixture::REVIEWER};
use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn only_leased_consumer_tasks_gain_the_consumer_span_field() {
    let h = Harness::new().await;
    let captured = Capture::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_span_events(tracing_subscriber::fmt::format::FmtSpan::NEW)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    for (event, routed, lease, consumer) in [
        ("legacy-case", false, false, None),
        ("finalizer-case", true, false, None),
        ("default-case", true, true, None),
        ("reviewer-case", true, true, Some(REVIEWER)),
    ] {
        let mut request = if routed {
            h.request(&["default", REVIEWER])
        } else {
            h.base.request(event)
        };
        request.activity.event_id = event.parse().unwrap();
        request.consumer = consumer.map(|id| cmaeg_core::ids::ConsumerId::parse(id).unwrap());
        if lease {
            *request.lease.lock().unwrap() = Some(
                h.capacity
                    .try_lease(
                        &request.channel.pointer.record_id,
                        &request.channel.definition.owner.subject,
                    )
                    .unwrap(),
            );
        }
        h.base.tracker.spawn(&request, async {}).unwrap();
    }
    idle(&h.base.tracker).await;
    let logs = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let lines = logs
        .lines()
        .filter(|line| line.contains("cmaeg.turn"))
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 4, "{logs}");
    for event in ["legacy-case", "finalizer-case"] {
        let line = lines.iter().find(|line| line.contains(event)).unwrap();
        assert!(!line.contains("consumer="), "{line}");
    }
    assert!(lines
        .iter()
        .find(|line| line.contains("default-case"))
        .unwrap()
        .contains("consumer=\"default\""));
    assert!(lines
        .iter()
        .find(|line| line.contains("reviewer-case"))
        .unwrap()
        .contains(&format!("consumer=\"{REVIEWER}\"")));
}
