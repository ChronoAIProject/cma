use super::*;
use mongodb::event::{cmap::CmapEvent, EventHandler};
use std::time::Instant;

#[derive(Default)]
pub(super) struct Usage {
    active: BTreeMap<u32, Instant>,
    peak: usize,
    seconds: f64,
}

pub(super) fn note_ownership(event: &mongodb::event::command::CommandEvent, counter: &AtomicUsize) {
    if let mongodb::event::command::CommandEvent::Started(event) = event {
        if event.command.values().any(|value| {
            matches!(
                value.as_str(),
                Some("cma_agent_manager_members" | "cma_agent_manager_owners")
            )
        }) {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    }
}

pub(super) fn observe(usage: Arc<Mutex<Usage>>) -> EventHandler<CmapEvent> {
    EventHandler::callback(move |event| {
        let mut usage = usage.lock().unwrap();
        match event {
            CmapEvent::ConnectionCheckedOut(event) => {
                assert!(usage
                    .active
                    .insert(event.connection_id, Instant::now())
                    .is_none());
                usage.peak = usage.peak.max(usage.active.len());
            }
            CmapEvent::ConnectionCheckedIn(event) => {
                let start = usage
                    .active
                    .remove(&event.connection_id)
                    .expect("matching checkout");
                usage.seconds += start.elapsed().as_secs_f64();
            }
            _ => {}
        }
    })
}

impl Usage {
    pub(super) fn reset(&mut self) {
        assert!(self.active.is_empty());
        self.peak = 0;
        self.seconds = 0.0;
    }

    pub(super) fn report(&self, deleting: bool) {
        assert!(
            self.active.is_empty(),
            "all tested control/worker driver calls joined"
        );
        println!(
            "CONTROL_LIFECYCLE_DEMAND {}",
            json!({
                "method": if deleting {"DELETE"} else {"STOP"},
                "pool":"fixture_control_client", "configured_pool_size":30,
                "peak_checked_out":self.peak, "connection_seconds":self.seconds,
                "scope":"control and real deferred worker I/O plus fixture readback; separate identity/ownership clients excluded",
                "hard_full_pool_bound_proven":false
            })
        );
    }
}

pub(super) async fn settle(usage: &Mutex<Usage>) {
    // A dropped connection is returned by the driver's pool worker. Joining a
    // command alone does not acknowledge that asynchronous check-in message.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !usage.lock().unwrap().active.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("all fixture connections must return to the pool");
}
