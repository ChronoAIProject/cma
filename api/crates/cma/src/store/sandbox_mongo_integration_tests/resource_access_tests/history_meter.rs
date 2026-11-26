use super::*;
use mongodb::event::cmap::CmapEvent;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Clone, Copy, Default)]
pub(super) struct OpeningStats {
    pub commands: u64,
    pub checkouts: u64,
    pub longest_checkout: Duration,
    pub longest_command: Duration,
}

#[derive(Clone, Default)]
pub(super) struct Stats {
    pub commands: BTreeMap<String, u64>,
    pub per_agent_heads: BTreeMap<String, Vec<std::time::Instant>>,
    pub checkouts: u64,
    pub snapshots: u64,
    pub snapshot_commands: u64,
    pub auth_commands: u64,
    pub docs: u64,
    pub bytes: u64,
    pub history_bytes: u64,
    snapshot_requests: BTreeMap<i32, Option<i64>>,
    snapshot_cursors: BTreeSet<i64>,
    opening: Option<OpeningStats>,
}

#[derive(Default)]
pub(super) struct Meter {
    pub stats: Mutex<Stats>,
}

impl Meter {
    pub fn clear(&self) {
        *self.stats.lock().unwrap() = Stats::default();
    }
    pub fn start_opening(&self) {
        self.stats.lock().unwrap().opening = Some(OpeningStats::default());
    }
    pub fn opening_stats(&self) -> OpeningStats {
        self.stats.lock().unwrap().opening.unwrap()
    }
    pub fn finish_opening(&self) {
        self.stats.lock().unwrap().opening = None;
    }
    pub async fn client(self: &Arc<Self>) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let observed = self.clone();
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if let CmapEvent::ConnectionCheckedOut(event) = event {
                let mut stats = observed.stats.lock().unwrap();
                if let Some(opening) = &mut stats.opening {
                    opening.checkouts += 1;
                    opening.longest_checkout = opening.longest_checkout.max(event.duration);
                } else {
                    stats.checkouts += 1;
                }
            }
        }));
        let observed = self.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| match event {
            CommandEvent::Started(event) => {
                let mut stats = observed.stats.lock().unwrap();
                if let Some(opening) = &mut stats.opening {
                    opening.commands += 1;
                    return;
                }
                let collection = if event.command_name == "getMore" {
                    event.command.get_str("collection")
                } else {
                    event.command.get_str(&event.command_name)
                }
                .unwrap_or("none");
                // Baseline routes have no phase spans. Derive fixed logical
                // phases from command metadata; never retain filters or payloads.
                let phase = if (event.command_name == "aggregate" && collection == "agents")
                    || collection == "resource_shares"
                    || event.command_name == "abortTransaction"
                {
                    "authorization"
                } else if collection == "agent_commands" {
                    "binding"
                } else {
                    "log"
                };
                let cursor = event.command.get_i64("getMore").ok();
                if matches!(event.command.get_bool("autocommit"), Ok(false))
                    || event
                        .command
                        .get_document("readConcern")
                        .is_ok_and(|concern| matches!(concern.get_str("level"), Ok("snapshot")))
                    || cursor.is_some_and(|id| stats.snapshot_cursors.contains(&id))
                {
                    stats.snapshot_commands += 1;
                    stats.snapshot_requests.insert(event.request_id, cursor);
                }
                if event.command_name == "aggregate" && collection == "agents" {
                    if let Some(ids) = event
                        .command
                        .get_array("pipeline")
                        .ok()
                        .and_then(|pipeline| pipeline.first())
                        .and_then(|stage| stage.as_document())
                        .and_then(|stage| stage.get_document("$match").ok())
                        .and_then(|filter| filter.get_document("_id").ok())
                        .and_then(|filter| filter.get_array("$in").ok())
                    {
                        for id in ids.iter().filter_map(|id| id.as_str()) {
                            stats
                                .per_agent_heads
                                .entry(id.into())
                                .or_default()
                                .push(std::time::Instant::now());
                        }
                    }
                }
                stats.snapshots +=
                    u64::from(event.command_name == "aggregate" && collection == "agents");
                stats.auth_commands += u64::from(phase == "authorization");
                *stats
                    .commands
                    .entry(format!("{phase}/{collection}/{}", event.command_name))
                    .or_default() += 1;
            }
            CommandEvent::Succeeded(event) => {
                let mut stats = observed.stats.lock().unwrap();
                // Setup needs counts and latency. Detailed samples re-encode
                // each reply document while holding the meter lock.
                if let Some(opening) = &mut stats.opening {
                    opening.longest_command = opening.longest_command.max(event.duration);
                    return;
                }
                let snapshot = stats.snapshot_requests.remove(&event.request_id);
                if let Some(Some(cursor)) = snapshot {
                    stats.snapshot_cursors.remove(&cursor);
                }
                if let Ok(cursor) = event.reply.get_document("cursor") {
                    if let Ok(id) = cursor.get_i64("id") {
                        if snapshot.is_some() && id != 0 {
                            stats.snapshot_cursors.insert(id);
                        }
                    }
                    let batch = cursor
                        .get_array("firstBatch")
                        .or_else(|_| cursor.get_array("nextBatch"))
                        .unwrap();
                    for row in batch {
                        stats.docs += 1;
                        let bytes = row.as_document().unwrap().to_vec().unwrap().len() as u64;
                        stats.bytes += bytes;
                        if matches!(event.command_name.as_str(), "find" | "getMore")
                            && cursor
                                .get_str("ns")
                                .is_ok_and(|ns| ns.ends_with(".agent_log"))
                        {
                            stats.history_bytes += bytes;
                        }
                    }
                }
            }
            CommandEvent::Failed(event) => {
                let mut stats = observed.stats.lock().unwrap();
                if let Some(opening) = &mut stats.opening {
                    opening.longest_command = opening.longest_command.max(event.duration);
                } else {
                    stats.snapshot_requests.remove(&event.request_id);
                }
            }
            _ => {}
        }));
        Client::with_options(options).unwrap()
    }
}
