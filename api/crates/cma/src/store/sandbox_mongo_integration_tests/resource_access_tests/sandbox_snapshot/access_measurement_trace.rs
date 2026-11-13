use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub(super) struct Stats {
    pub commands: BTreeMap<String, u64>,
    pub docs: u64,
    pub bytes: u64,
    pub agent_docs: u64,
    pub agent_bytes: u64,
}
impl Stats {
    pub fn count(&self) -> u64 {
        self.commands.values().sum()
    }
}

#[derive(Default)]
pub(super) struct Meter {
    pub stats: Mutex<Stats>,
    pub checkouts: AtomicU64,
    pending: Mutex<BTreeMap<i32, (String, String)>>,
}
impl Meter {
    pub fn clear(&self) {
        *self.stats.lock().unwrap() = Stats::default();
        self.checkouts.store(0, Ordering::SeqCst);
        assert!(self.pending.lock().unwrap().is_empty());
    }
}

pub(super) async fn client(meter: &Arc<Meter>) -> Client {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let trace = meter.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            trace.checkouts.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let trace = meter.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| match event {
        CommandEvent::Started(event) => {
            let phase = layer2_core::diagnostic_context::DiagnosticContext::current()
                .phase
                .to_owned();
            let collection = if event.command_name == "getMore" {
                event.command.get_str("collection")
            } else {
                event.command.get_str(&event.command_name)
            }
            .unwrap_or("none")
            .to_owned();
            let key = format!("{phase}/{collection}/{}", event.command_name);
            *trace.stats.lock().unwrap().commands.entry(key).or_default() += 1;
            trace
                .pending
                .lock()
                .unwrap()
                .insert(event.request_id, (phase, collection));
        }
        CommandEvent::Succeeded(event) => {
            let Some((phase, collection)) = trace.pending.lock().unwrap().remove(&event.request_id)
            else {
                return;
            };
            if let Ok(cursor) = event.reply.get_document("cursor") {
                let rows = cursor
                    .get_array("firstBatch")
                    .or_else(|_| cursor.get_array("nextBatch"))
                    .unwrap();
                let mut stats = trace.stats.lock().unwrap();
                for row in rows {
                    let row = row.as_document().unwrap();
                    stats.docs += 1;
                    stats.bytes += row.to_vec().unwrap().len() as u64;
                    if phase == "hierarchy" {
                        let agent = if collection == "agents" {
                            Some(row)
                        } else if matches!(row.get_str("kind"), Ok("agent")) {
                            Some(row.get_document("value").unwrap())
                        } else {
                            None
                        };
                        if let Some(agent) = agent {
                            stats.agent_docs += 1;
                            stats.agent_bytes += agent.to_vec().unwrap().len() as u64;
                        }
                    }
                }
            }
        }
        CommandEvent::Failed(event) => {
            trace.pending.lock().unwrap().remove(&event.request_id);
        }
        _ => {}
    }));
    Client::with_options(options).unwrap()
}
