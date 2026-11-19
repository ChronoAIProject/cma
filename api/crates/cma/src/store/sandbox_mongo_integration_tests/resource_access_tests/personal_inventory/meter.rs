use super::*;
use std::{collections::BTreeMap, time::Instant};

#[derive(Clone, Debug, Default)]
pub(super) struct Stats {
    pub commands: BTreeMap<String, usize>,
    pub bytes: usize,
    pub documents: usize,
    pub failed: usize,
    pub checkouts: usize,
    pub checkout_failed: usize,
    pub command_us: BTreeMap<String, u128>,
    pub checkout_us: u128,
    pub connections_ready: usize,
    pub peak_in_use: usize,
    pub hold_us: u128,
    pub checkout_p95_us: u128,
}
impl Stats {
    pub fn count(&self) -> usize {
        self.commands.values().sum()
    }
}
#[derive(Default)]
pub(super) struct Meter {
    stats: Mutex<Stats>,
    replies: Mutex<Vec<Document>>,
    pending: Mutex<BTreeMap<i32, String>>,
    holds: Mutex<BTreeMap<u32, Instant>>,
    waits: Mutex<Vec<u128>>,
}
impl Meter {
    pub fn take(&self) -> Stats {
        let mut stats = std::mem::take(&mut *self.stats.lock().unwrap());
        let mut waits = std::mem::take(&mut *self.waits.lock().unwrap());
        waits.sort_unstable();
        if !waits.is_empty() {
            stats.checkout_p95_us = waits[(waits.len() * 95).div_ceil(100) - 1];
        }
        // Byte accounting is exact, but serializing reply copies is measurement
        // work, not request work. Do it after the measured requests have settled.
        for reply in std::mem::take(&mut *self.replies.lock().unwrap()) {
            stats.bytes += reply.to_vec().unwrap().len();
        }
        stats
    }
    pub async fn client(self: &Arc<Self>, app: &str) -> Client {
        self.client_with_command_observer(app, true).await
    }

    pub async fn client_without_command_observer(self: &Arc<Self>, app: &str) -> Client {
        self.client_with_command_observer(app, false).await
    }

    async fn client_with_command_observer(
        self: &Arc<Self>,
        app: &str,
        observe_commands: bool,
    ) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.app_name = Some(app.into());
        options.max_pool_size = Some(
            std::env::var("CMA_LIST_TEST_POOL_SIZE")
                .ok()
                .map(|v| v.parse().unwrap())
                .unwrap_or(100),
        );
        if observe_commands {
            let meter = self.clone();
            options.command_event_handler =
                Some(EventHandler::callback(move |event| match event {
                    CommandEvent::Started(event) => {
                        let collection = event
                            .command
                            .get_str(if event.command_name == "getMore" {
                                "collection"
                            } else {
                                &event.command_name
                            })
                            .unwrap_or("none");
                        meter.pending.lock().unwrap().insert(
                            event.request_id,
                            format!(
                                "{collection}/{}/{}",
                                event.command_name,
                                event.command.get_str("comment").unwrap_or("none")
                            ),
                        );
                        *meter
                            .stats
                            .lock()
                            .unwrap()
                            .commands
                            .entry(format!("{collection}/{}", event.command_name))
                            .or_default() += 1;
                    }
                    CommandEvent::Succeeded(event) => {
                        let mut stats = meter.stats.lock().unwrap();
                        *stats
                            .command_us
                            .entry(
                                meter
                                    .pending
                                    .lock()
                                    .unwrap()
                                    .remove(&event.request_id)
                                    .unwrap(),
                            )
                            .or_default() += event.duration.as_micros();
                        if let Ok(cursor) = event.reply.get_document("cursor") {
                            stats.documents += cursor
                                .get_array("firstBatch")
                                .or_else(|_| cursor.get_array("nextBatch"))
                                .unwrap()
                                .len();
                        }
                        drop(stats);
                        meter.replies.lock().unwrap().push(event.reply);
                    }
                    CommandEvent::Failed(event) => {
                        meter.pending.lock().unwrap().remove(&event.request_id);
                        meter.stats.lock().unwrap().failed += 1;
                        eprintln!(
                            "LIST_DRIVER_FAILURE command={} duration={:?} error={:?}",
                            event.command_name,
                            event.duration,
                            match event.failure.kind.as_ref() {
                                mongodb::error::ErrorKind::Command(e) => Some(e.code),
                                _ => None,
                            }
                        );
                    }
                    _ => {}
                }));
        } else {
            crate::store::mongo_latency::observe(&mut options);
        }
        let meter = self.clone();
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            use mongodb::event::cmap::CmapEvent;
            let mut stats = meter.stats.lock().unwrap();
            match event {
                CmapEvent::ConnectionCheckedOut(event) => {
                    stats.checkouts += 1;
                    stats.checkout_us += event.duration.as_micros();
                    meter.waits.lock().unwrap().push(event.duration.as_micros());
                    let mut holds = meter.holds.lock().unwrap();
                    assert!(holds.insert(event.connection_id, Instant::now()).is_none());
                    stats.peak_in_use = stats.peak_in_use.max(holds.len());
                }
                CmapEvent::ConnectionCheckedIn(event) => {
                    if let Some(start) = meter.holds.lock().unwrap().remove(&event.connection_id) {
                        stats.hold_us += start.elapsed().as_micros();
                    }
                }
                CmapEvent::ConnectionClosed(event) => {
                    meter.holds.lock().unwrap().remove(&event.connection_id);
                }
                CmapEvent::ConnectionReady(_) => stats.connections_ready += 1,
                CmapEvent::ConnectionCheckoutFailed(_) => stats.checkout_failed += 1,
                _ => {}
            }
        }));
        Client::with_options(options).unwrap()
    }
}
