use mongodb::event::command::CommandEvent;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

pub(crate) struct CommandCounter {
    commands: AtomicUsize,
    capability_reads: AtomicUsize,
    capability_ids: Mutex<std::collections::BTreeSet<String>>,
}
impl Default for CommandCounter {
    fn default() -> Self {
        Self {
            commands: AtomicUsize::new(0),
            capability_reads: AtomicUsize::new(0),
            capability_ids: Mutex::new(Default::default()),
        }
    }
}
impl CommandCounter {
    pub(crate) fn observe(&self, event: CommandEvent) {
        let CommandEvent::Started(event) = event else {
            return;
        };
        self.commands.fetch_add(1, Ordering::Relaxed);
        if event.command_name == "find"
            && event
                .command
                .get_document("filter")
                .ok()
                .and_then(|filter| filter.get_str("_id").ok())
                .is_some_and(|id| self.capability_ids.lock().unwrap().contains(id))
        {
            self.capability_reads.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub(crate) fn add_capability(&self, id: String) {
        self.capability_ids.lock().unwrap().insert(id);
    }
    pub(crate) fn counts(&self) -> (usize, usize) {
        (
            self.commands.load(Ordering::Relaxed),
            self.capability_reads.load(Ordering::Relaxed),
        )
    }
}

pub(crate) fn handler(counter: Arc<CommandCounter>) -> mongodb::event::EventHandler<CommandEvent> {
    mongodb::event::EventHandler::callback(move |event| counter.observe(event))
}
