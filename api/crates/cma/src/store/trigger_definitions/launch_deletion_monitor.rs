use super::*;
use mongodb::{
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use std::sync::{Condvar, Mutex};
use tokio::sync::Notify;

pub(super) struct Monitor {
    commands: Mutex<Vec<Document>>,
    pause_subject: Mutex<Option<String>>,
    entered: Notify,
    release: (Mutex<bool>, Condvar),
}

impl Monitor {
    pub(super) fn commands(&self) -> Vec<Document> {
        self.commands.lock().unwrap().clone()
    }

    pub(super) fn pause_fence(&self, subject: &SubjectId) {
        *self.pause_subject.lock().unwrap() = Some(subject.as_str().to_owned());
    }

    pub(super) async fn paused(&self) {
        tokio::time::timeout(std::time::Duration::from_secs(20), self.entered.notified())
            .await
            .expect("Launch reached the visitor fence");
    }

    pub(super) fn resume(&self) {
        *self.release.0.lock().unwrap() = true;
        self.release.1.notify_one();
    }

    fn should_pause(&self, command: &Document) -> bool {
        if command.get_str("update").ok() != Some("sandbox_admissions") {
            return false;
        }
        let mut subject = self.pause_subject.lock().unwrap();
        let matches = command.get_array("updates").unwrap().iter().any(|update| {
            update
                .as_document()
                .and_then(|update| update.get_document("q").ok())
                .and_then(|query| query.get_str("_id").ok())
                == subject.as_deref()
        });
        matches && subject.take().is_some()
    }
}

pub(super) async fn monitored(seed: &Seeded) -> (MongoTriggerDefinitionStore, Arc<Monitor>) {
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let monitor = Arc::new(Monitor {
        commands: Mutex::new(Vec::new()),
        pause_subject: Mutex::new(None),
        entered: Notify::new(),
        release: (Mutex::new(false), Condvar::new()),
    });
    let observed = monitor.clone();
    let database = seed.database.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.database_name == database {
                observed
                    .commands
                    .lock()
                    .unwrap()
                    .push(event.command.clone());
                if observed.should_pause(&event.command) {
                    observed.entered.notify_one();
                    let released = observed
                        .release
                        .1
                        .wait_timeout_while(
                            observed.release.0.lock().unwrap(),
                            std::time::Duration::from_secs(30),
                            |released| !*released,
                        )
                        .unwrap();
                    assert!(*released.0, "test released the visitor fence");
                }
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = MongoTriggerDefinitionStore::open(&client, &seed.database, seed.profiles.clone())
        .await
        .unwrap();
    monitor.commands.lock().unwrap().clear();
    (store, monitor)
}
