use super::*;

#[derive(Default)]
pub(super) struct Counts {
    pub commands: usize,
    pub reply_bytes: usize,
    pub checkpoint_reads: usize,
    pub failed: usize,
}
impl Counts {
    pub fn add(&mut self, other: Self) {
        self.commands += other.commands;
        self.reply_bytes += other.reply_bytes;
        self.checkpoint_reads += other.checkpoint_reads;
        self.failed += other.failed;
    }
}

/// Count actual driver commands and complete BSON replies during public reads.
/// No filters, credentials or retained text are stored in measurement results.
#[derive(Default)]
pub(super) struct Meter(Mutex<Option<Counts>>);
impl Meter {
    pub fn start(&self) {
        assert!(self.0.lock().unwrap().replace(Counts::default()).is_none());
    }
    pub fn finish(&self) -> Counts {
        self.0.lock().unwrap().take().unwrap()
    }
    pub async fn client(self: &Arc<Self>) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let observed = self.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            let mut held = observed.0.lock().unwrap();
            let Some(stats) = held.as_mut() else { return };
            match event {
                CommandEvent::Started(event) => {
                    stats.commands += 1;
                    stats.checkpoint_reads += usize::from(
                        event.command_name == "find"
                            && event
                                .command
                                .get_document("projection")
                                .is_ok_and(|value| value.contains_key("agent_capacity_prefix_v1")),
                    );
                }
                CommandEvent::Succeeded(event) => {
                    stats.reply_bytes += event.reply.to_vec().unwrap().len();
                }
                CommandEvent::Failed(_) => stats.failed += 1,
                _ => {}
            }
        }));
        Client::with_options(options).unwrap()
    }
}
