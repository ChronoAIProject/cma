use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use layer2_core::{
    admission::{AdmissionCeilings, AdmissionCeilingsStore},
    SubjectId,
};
use mongodb::{
    bson::{doc, Document},
    event::{command::CommandEvent, EventHandler},
    options::ClientOptions,
    Client,
};

#[derive(Clone, Copy)]
pub(in crate::store) enum Point {
    Snapshot,
    Owner,
    Fence,
    CommitAck,
    ConsentAck,
}

#[derive(Default)]
struct State {
    point: Option<Point>,
    request: Option<i32>,
    commands: Vec<Document>,
}

#[derive(Default)]
pub(in crate::store) struct Pause {
    state: Mutex<State>,
    reached: tokio::sync::Notify,
    released: Mutex<bool>,
    wake: Condvar,
}

pub(in crate::store) struct Release(Arc<Pause>);

impl Drop for Release {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl Pause {
    pub(in crate::store) async fn client(self: &Arc<Self>) -> Client {
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let pause = self.clone();
        options.command_event_handler =
            Some(EventHandler::callback(move |event| pause.observe(event)));
        Client::with_options(options).unwrap()
    }

    pub(in crate::store) fn arm(self: &Arc<Self>, point: Point) -> Release {
        *self.state.lock().unwrap() = State {
            point: Some(point),
            ..Default::default()
        };
        *self.released.lock().unwrap() = false;
        Release(self.clone())
    }

    pub(in crate::store) async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(20), self.reached.notified())
            .await
            .expect("admission did not reach the controlled command response");
    }

    pub(in crate::store) fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.wake.notify_all();
    }

    pub(in crate::store) fn attempts(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
            .count()
    }

    pub(in crate::store) fn commits(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| command.contains_key("commitTransaction"))
            .count()
    }

    fn observe(&self, event: CommandEvent) {
        let should_pause = {
            let mut state = self.state.lock().unwrap();
            match event {
                CommandEvent::Started(event) => {
                    let command = &event.command;
                    let matches = match state.point {
                        Some(Point::Snapshot) => {
                            command.get_bool("startTransaction").ok() == Some(true)
                        }
                        Some(Point::Owner) => {
                            command.get_str("update").ok() == Some("sandbox_admissions")
                                && command.contains_key("autocommit")
                        }
                        Some(Point::Fence) => {
                            command.get_str("update").ok() == Some("platform_settings")
                                && command.contains_key("autocommit")
                        }
                        Some(Point::CommitAck) => event.command_name == "commitTransaction",
                        Some(Point::ConsentAck) => command.get_str("update").ok() == Some("users"),
                        None => false,
                    };
                    if matches {
                        state.point = None;
                        state.request = Some(event.request_id);
                    }
                    state.commands.push(event.command);
                    false
                }
                CommandEvent::Succeeded(event) if state.request == Some(event.request_id) => {
                    state.request = None;
                    true
                }
                _ => false,
            }
        };
        if should_pause {
            self.reached.notify_one();
            let (_guard, timeout) = self
                .wake
                .wait_timeout_while(
                    self.released.lock().unwrap(),
                    Duration::from_secs(30),
                    |released| !*released,
                )
                .unwrap();
            assert!(
                !timeout.timed_out(),
                "test did not release admission response"
            );
        }
    }
}

pub(in crate::store) async fn set_ceilings(
    client: &Client,
    name: &str,
    ceilings: AdmissionCeilings,
) {
    let operator = crate::store::MongoAdmissionCeilingsStore::open(client, name, 0)
        .await
        .unwrap();
    let revision = operator.read().await.unwrap().revision;
    operator
        .replace(
            revision,
            &SubjectId::new("cutoff-operator"),
            "adjust admission cap",
            ceilings,
            100 + revision,
        )
        .await
        .unwrap();
}

pub(in crate::store) async fn platform(client: &Client, name: &str) -> Document {
    client
        .database(name)
        .collection::<Document>("platform_settings")
        .find_one(doc! {"_id": "platform"})
        .await
        .unwrap()
        .unwrap()
}
