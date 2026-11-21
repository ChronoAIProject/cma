use super::*;
use std::sync::{Arc, Mutex};
use tracing::{
    span::{Attributes, Id, Record},
    Event, Metadata, Subscriber,
};

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);
impl Subscriber for Capture {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        assert!(
            event.is_root(),
            "child timing must not inherit caller content"
        );
        struct Fields(String);
        impl tracing::field::Visit for Fields {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                self.0.push_str(&format!("{}={value:?} ", field.name()));
            }
        }
        let mut fields = Fields(String::new());
        event.record(&mut fields);
        self.0.lock().unwrap().push(fields.0);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[tokio::test(start_paused = true)]
async fn child_timings_use_fixed_categories_and_count_success_failure_and_deadline() {
    let capture = Capture::default();
    let _guard = tracing::subscriber::set_default(capture.clone());
    let span = tracing::info_span!("caller", private = "fixture private content");
    let _entered = span.enter();
    request(Child::RouteList, async {
        tokio::time::sleep(Duration::from_secs(2)).await;
        Ok(())
    })
    .await
    .unwrap();
    assert!(request::<()>(Child::ClaimRead, async {
        Err(AdapterError::Rejected("fixture error content".into()))
    })
    .await
    .is_err());
    assert!(request::<()>(Child::AttachmentRead, std::future::pending())
        .await
        .is_err());
    assert!(tokio::time::timeout(
        Duration::from_secs(3),
        request::<()>(Child::ClaimRead, std::future::pending())
    )
    .await
    .is_err());
    let rows = capture.0.lock().unwrap();
    assert_eq!(rows.len(), 4);
    for (row, category, outcome, duration) in [
        (&rows[0], "route_list", "complete", 2000),
        (&rows[1], "claim_read", "unavailable", 0),
        (&rows[2], "attachment_read", "deadline", 5000),
        (&rows[3], "claim_read", "cancelled", 3000),
    ] {
        assert!(row.contains(&format!("child=\"{category}\"")), "{row}");
        assert!(row.contains(&format!("outcome=\"{outcome}\"")), "{row}");
        assert!(row.contains(&format!("elapsed_ms={duration}")), "{row}");
        assert!(row.contains("count=1"));
        assert!(!row.contains("fixture"));
    }
}
