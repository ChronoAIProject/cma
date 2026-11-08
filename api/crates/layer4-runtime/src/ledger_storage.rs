//! Bounded SQLite ownership. The worker owns the connection and lifetime lock.
//! No database error or path is exposed through operational diagnostics.
mod connection;
mod paths;
mod probe;
pub(crate) mod reserve;
#[cfg(test)]
mod reserve_tests;
#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};

use crate::failure::{Result, RuntimeError};
use rusqlite::Connection;
use tokio::sync::oneshot;

pub(crate) use probe::run as probe;

const QUEUE_LIMIT: usize = 8;
const MAX_VALUE_BYTES: i32 = 2 * 1024 * 1024;

type Work = Box<dyn FnOnce(&mut Connection) + Send>;
enum Message {
    Work(Work),
    Close(oneshot::Sender<Result<()>>),
}

pub(crate) struct Storage {
    sender: SyncSender<Message>,
    path: PathBuf,
}

impl Storage {
    pub(crate) async fn open(path: PathBuf, create: bool, pages: u32) -> Result<Self> {
        let (sender, receiver) = sync_channel(QUEUE_LIMIT);
        let (opened, ready) = oneshot::channel();
        let directory = path.clone();
        std::thread::Builder::new()
            .name("runtime-ledger".into())
            .spawn(move || {
                let (mut connection, lock) = match connection::open(&path, create, pages) {
                    Ok(opened) => opened,
                    Err(error) => {
                        let _ = opened.send(Err(error));
                        return;
                    }
                };
                if opened.send(Ok(())).is_err() {
                    return;
                }
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Work(work) => work(&mut connection),
                        Message::Close(reply) => {
                            let result = connection.close().map_err(|_| storage_error());
                            drop(lock);
                            let _ = reply.send(result);
                            return;
                        }
                    }
                }
            })
            .map_err(|_| storage_error())?;
        ready.await.map_err(|_| storage_error())??;
        Ok(Self {
            sender,
            path: directory,
        })
    }

    pub(crate) async fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, result) = oneshot::channel();
        let directory = self.path.clone();
        self.send(Message::Work(Box::new(move |connection| {
            // Finish an admitted transaction even if its caller disconnects.
            // A cancelled wait never implies that the transaction did not commit.
            let outcome = work(connection);
            if outcome
                .as_ref()
                .is_err_and(|error| error.code == "mutation_ledger_storage_hold")
            {
                // Do not retry the transaction or effect after an ambiguous error.
                // Free physical headroom for a subsequent durable-state inspection.
                // The zero-length reserve persists the new-admission hold on restart.
                let _ = reserve::release(&directory);
            }
            let _ = reply.send(outcome);
        })))?;
        result.await.map_err(|_| storage_error())?
    }

    pub(crate) async fn close(self) -> Result<()> {
        let (reply, result) = oneshot::channel();
        self.send(Message::Close(reply))?;
        result.await.map_err(|_| storage_error())?
    }

    fn send(&self, message: Message) -> Result<()> {
        self.sender.try_send(message).map_err(|error| match error {
            TrySendError::Full(_) => RuntimeError::rejected(
                "mutation_ledger_busy",
                "runtime mutation ledger worker capacity is exhausted",
            ),
            TrySendError::Disconnected(_) => storage_error(),
        })
    }
}

fn storage_error() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_storage_hold",
        "runtime mutation ledger storage requires verification",
    )
}

fn invalid() -> RuntimeError {
    RuntimeError::rejected(
        "mutation_ledger_storage_invalid",
        "runtime mutation ledger storage contract is invalid",
    )
}
