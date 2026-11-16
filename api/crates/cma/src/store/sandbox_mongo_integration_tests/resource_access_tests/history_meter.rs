use super::*;
use mongodb::event::cmap::CmapEvent;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct Stats {
    pub commands: BTreeMap<String, u64>,
    pub checkouts: u64,
    pub snapshots: u64,
    pub auth_commands: u64,
    pub docs: u64,
    pub bytes: u64,
}

#[derive(Default)]
pub(super) struct Meter {
    pub stats: Mutex<Stats>,
}

impl Meter {
    pub fn clear(&self) {
        *self.stats.lock().unwrap() = Stats::default();
    }
    pub async fn client(self: &Arc<Self>) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let observed = self.clone();
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
                observed.stats.lock().unwrap().checkouts += 1;
            }
        }));
        let observed = self.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| match event {
            CommandEvent::Started(event) => {
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
                let mut stats = observed.stats.lock().unwrap();
                stats.snapshots +=
                    u64::from(event.command_name == "aggregate" && collection == "agents");
                stats.auth_commands += u64::from(phase == "authorization");
                *stats
                    .commands
                    .entry(format!("{phase}/{collection}/{}", event.command_name))
                    .or_default() += 1;
            }
            CommandEvent::Succeeded(event) => {
                if let Ok(cursor) = event.reply.get_document("cursor") {
                    let batch = cursor
                        .get_array("firstBatch")
                        .or_else(|_| cursor.get_array("nextBatch"))
                        .unwrap();
                    let mut stats = observed.stats.lock().unwrap();
                    for row in batch {
                        stats.docs += 1;
                        stats.bytes += row.as_document().unwrap().to_vec().unwrap().len() as u64;
                    }
                }
            }
            _ => {}
        }));
        Client::with_options(options).unwrap()
    }
}
