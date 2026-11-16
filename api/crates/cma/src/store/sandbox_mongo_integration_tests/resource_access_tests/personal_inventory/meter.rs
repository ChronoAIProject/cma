use super::*;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub(super) struct Stats {
    pub commands: BTreeMap<String, usize>,
    pub bytes: usize,
    pub documents: usize,
    pub failed: usize,
    pub checkouts: usize,
    pub command_us: BTreeMap<String, u128>,
    pub checkout_us: u128,
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
}
impl Meter {
    pub fn take(&self) -> Stats {
        let mut stats = std::mem::take(&mut *self.stats.lock().unwrap());
        // Byte accounting is exact, but serializing reply copies is measurement
        // work, not request work. Do it after the measured requests have settled.
        for reply in std::mem::take(&mut *self.replies.lock().unwrap()) {
            stats.bytes += reply.to_vec().unwrap().len();
        }
        stats
    }
    pub async fn client(self: &Arc<Self>, app: &str) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.app_name = Some(app.into());
        options.max_pool_size = Some(100);
        let meter = self.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| match event {
            CommandEvent::Started(event) => {
                let collection = event
                    .command
                    .get_str(if event.command_name == "getMore" {
                        "collection"
                    } else {
                        &event.command_name
                    })
                    .unwrap_or("none");
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
                *stats.command_us.entry(event.command_name).or_default() +=
                    event.duration.as_micros();
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
            CommandEvent::Failed(_) => meter.stats.lock().unwrap().failed += 1,
            _ => {}
        }));
        let meter = self.clone();
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if let mongodb::event::cmap::CmapEvent::ConnectionCheckedOut(event) = event {
                let mut stats = meter.stats.lock().unwrap();
                stats.checkouts += 1;
                stats.checkout_us += event.duration.as_micros();
            }
        }));
        Client::with_options(options).unwrap()
    }
}
