use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use mongodb::{bson::Document, event::command::CommandEvent};

#[derive(Clone, Copy)]
pub(super) enum Point {
    ParentRead,
    ParentWrite,
    CommitAck,
}

#[derive(Default)]
struct State {
    point: Option<Point>,
    request: Option<i32>,
    commands: Vec<Document>,
}

#[derive(Default)]
pub(super) struct Gate {
    state: Mutex<State>,
    reached: tokio::sync::Notify,
    released: Mutex<bool>,
    wake: Condvar,
}

pub(super) struct Release(Arc<Gate>);

impl Drop for Release {
    fn drop(&mut self) {
        *self.0.released.lock().unwrap() = true;
        self.0.wake.notify_all();
    }
}

impl Gate {
    pub(super) fn arm(self: &Arc<Self>, point: Point) -> Release {
        *self.state.lock().unwrap() = State {
            point: Some(point),
            ..Default::default()
        };
        *self.released.lock().unwrap() = false;
        Release(self.clone())
    }

    pub(super) async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(15), self.reached.notified())
            .await
            .expect("suggestion transaction did not reach its controlled response");
    }

    pub(super) fn observe(&self, event: CommandEvent) {
        let pause = {
            let mut state = self.state.lock().unwrap();
            match event {
                CommandEvent::Started(event) => {
                    let command = &event.command;
                    let matches = command.contains_key("autocommit")
                        && match state.point {
                            Some(Point::ParentRead) => {
                                command.get_str("find").ok() == Some("sandboxes")
                            }
                            Some(Point::ParentWrite) => {
                                command.get_str("update").ok() == Some("sandboxes")
                            }
                            Some(Point::CommitAck) => event.command_name == "commitTransaction",
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
        if pause {
            self.reached.notify_one();
            let (_guard, timeout) = self
                .wake
                .wait_timeout_while(
                    self.released.lock().unwrap(),
                    Duration::from_secs(25),
                    |released| !*released,
                )
                .unwrap();
            assert!(
                !timeout.timed_out(),
                "test must release its transaction response"
            );
        }
    }

    pub(super) fn attempts(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| command.get_bool("startTransaction").ok() == Some(true))
            .count()
    }

    pub(super) fn release_marker_reads(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| {
                command
                    .get_document("projection")
                    .and_then(|projection| projection.get_i32("released_at_ms"))
                    .ok()
                    == Some(1)
            })
            .count()
    }

    pub(super) fn commits(&self) -> Vec<Document> {
        self.state
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| command.contains_key("commitTransaction"))
            .cloned()
            .collect()
    }
}
