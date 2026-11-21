//! Gate the real Mongo driver's commit dispatch to inspect admission ordering.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::Duration;

use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::{options::ClientOptions, Client};
use tokio::sync::Notify;

#[derive(Default)]
pub(super) struct CommitGate {
    armed: AtomicBool,
    pub entered: Notify,
    released: Mutex<bool>,
    resume: Condvar,
}

impl CommitGate {
    pub async fn client(self: &Arc<Self>) -> Client {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("CHRONO_TEST_MONGODB_URI must name a MongoDB replica set");
        let mut options = ClientOptions::parse(uri).await.unwrap();
        let gate = self.clone();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.command_name == "commitTransaction"
                    && gate.armed.swap(false, Ordering::SeqCst)
                {
                    gate.entered.notify_one();
                    // Only the dedicated multi-thread test client's commit is gated.
                    let (_released, timeout) = gate
                        .resume
                        .wait_timeout_while(
                            gate.released.lock().unwrap(),
                            Duration::from_secs(10),
                            |released| !*released,
                        )
                        .unwrap();
                    assert!(!timeout.timed_out(), "test must release Mongo commit gate");
                }
            }
        }));
        Client::with_options(options).unwrap()
    }

    pub fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    pub fn release(&self) {
        *self.released.lock().unwrap() = true;
        self.resume.notify_one();
    }
}
