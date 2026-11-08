//! Bounded SQLite ownership. The worker owns the connection and lifetime lock.
//! No database error or path is exposed through operational diagnostics.
pub(crate) mod anchored;
mod connection;
#[cfg(test)]
pub(crate) mod fault_vfs;
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

// Field order closes SQLite before unregistering its descriptor VFS even if a
// work callback panics. Close acknowledgement follows destruction of all locks.
struct WorkerConnection {
    database: Option<Connection>,
    _vfs: Option<anchored::Vfs>,
    _lock: std::fs::File,
    _outer: Option<std::sync::Arc<std::fs::File>>,
}
impl WorkerConnection {
    fn close(mut self) -> Result<()> {
        self.database
            .take()
            .ok_or_else(storage_error)?
            .close()
            .map_err(|_| storage_error())
    }
}

pub(crate) struct Storage {
    sender: SyncSender<Message>,
    path: PathBuf,
    anchor: Option<std::sync::Arc<std::fs::File>>,
}

impl Storage {
    pub(crate) async fn open(path: PathBuf, create: bool, pages: u32) -> Result<Self> {
        Self::open_named(path, create, pages, None, None, None).await
    }

    #[cfg(test)]
    pub(crate) async fn open_fault_vfs(
        path: PathBuf,
        create: bool,
        pages: u32,
        name: String,
    ) -> Result<Self> {
        Self::open_named(path, create, pages, Some(name), None, None).await
    }

    pub(crate) async fn open_with_lock(
        path: PathBuf,
        pages: u32,
        lock: std::sync::Arc<std::fs::File>,
    ) -> Result<Self> {
        Self::open_named(path, false, pages, None, Some(lock), None).await
    }

    pub(crate) async fn open_anchored(
        directory: std::sync::Arc<std::fs::File>,
        pages: u32,
    ) -> Result<Self> {
        Self::open_named(PathBuf::new(), true, pages, None, None, Some(directory)).await
    }

    async fn open_named(
        path: PathBuf,
        create: bool,
        pages: u32,
        vfs: Option<String>,
        outer_lock: Option<std::sync::Arc<std::fs::File>>,
        anchor: Option<std::sync::Arc<std::fs::File>>,
    ) -> Result<Self> {
        let (sender, receiver) = sync_channel(QUEUE_LIMIT);
        let (opened, ready) = oneshot::channel();
        let directory = path.clone();
        let retained_anchor = anchor.clone();
        std::thread::Builder::new()
            .name("runtime-ledger".into())
            .spawn(move || {
                let outer_lock = outer_lock;
                let connection = match anchor {
                    Some(directory) => anchored::open(directory, pages)
                        .map(|(db, lock, vfs)| (db, lock, Some(vfs))),
                    None => connection::open(&path, create, pages, vfs.as_deref())
                        .map(|(db, lock)| (db, lock, None)),
                };
                let (database, lock, anchored_vfs) = match connection {
                    Ok(opened) => opened,
                    Err(error) => {
                        if opened.send(Err(error)).is_err() {
                            tracing::error!(
                                phase = "open_reply",
                                "ledger caller disconnected during open refusal"
                            );
                        }
                        return;
                    }
                };
                let mut connection = WorkerConnection {
                    database: Some(database),
                    _vfs: anchored_vfs,
                    _lock: lock,
                    _outer: outer_lock,
                };
                if opened.send(Ok(())).is_err() {
                    return;
                }
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Work(work) => work(
                            connection
                                .database
                                .as_mut()
                                .expect("worker owns its connection"),
                        ),
                        Message::Close(reply) => {
                            let result = connection.close();
                            let _ = reply.send(result);
                            return;
                        }
                    }
                }
                drop(connection);
            })
            .map_err(|_| storage_error())?;
        ready.await.map_err(|_| storage_error())??;
        Ok(Self {
            sender,
            path: directory,
            anchor: retained_anchor,
        })
    }

    pub(crate) async fn call<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (reply, result) = oneshot::channel();
        let directory = self.path.clone();
        let anchor = self.anchor.clone();
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
                let released = match &anchor {
                    Some(directory) => anchored::release(directory),
                    None => reserve::release(&directory),
                };
                if let Err(error) = released {
                    tracing::error!(code = %error.code, phase = "release_reserve", "ledger cleanup failed");
                }
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

    /// A rejected open must release its writer before the refusal is observable.
    /// Dropping only the sender lets a subsequent open race worker shutdown.
    pub(crate) async fn validate(
        self,
        work: impl FnOnce(&mut Connection) -> Result<()> + Send + 'static,
    ) -> Result<Self> {
        if let Err(original) = self.call(work).await {
            if let Err(close) = self.close().await {
                tracing::error!(code = %close.code, "ledger close failed after open validation refusal");
            }
            return Err(original);
        }
        Ok(self)
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
