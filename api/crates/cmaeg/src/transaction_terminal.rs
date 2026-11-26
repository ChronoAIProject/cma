//! Commit and abort that a cancelled caller cannot strand (#3299).
//!
//! The pinned driver records a transaction as committed or aborted before it
//! sends `commitTransaction` or `abortTransaction`. If the future is dropped
//! during that send, for example while it waits for a pooled connection,
//! session drop sees a finished transaction and only returns the server
//! session to the pool. The server transaction then keeps its write locks
//! until that session is reused or the server's transaction lifetime expires.
//!
//! These wrappers arm a guard for the duration of the terminal command. A
//! guard dropped before the command completes ends the server session with
//! `killSessions`, which aborts its open transaction and releases its locks. A
//! dropped commit was already of unknown outcome to its caller; killing the
//! session resolves it to either committed or aborted.
//!
//! The kill runs on the session's own client on purpose: its implicit session
//! takes the cancelled lsid back off the LIFO pool, so no other operation can
//! reuse that lsid while the kill is in flight. A same-client operation may
//! still take the lsid first, before the kill checks out a session: a
//! transaction or retryable write then aborts the stranded transaction with
//! its newer txnNumber, while a plain read does not. Either way the late kill
//! interrupts that one operation once (11601), which its caller sees as an
//! ordinary store failure. Closing that window needs the driver to send the
//! terminal command from its own drop path (#3288).
//!
//! This is the gateway's own copy of the Agent Manager guard (#3288): the
//! crates may not depend on each other, and both keep the same semantics.

use std::time::Duration;

use mongodb::{
    bson::{doc, Document},
    error::Result,
    Client, ClientSession,
};
use tracing::Instrument as _;

/// Bounds the detached `killSessions` so a lost primary cannot pin the task.
const KILL_DEADLINE: Duration = Duration::from_secs(10);

/// Guarded terminal commands for every gateway store transaction.
pub(crate) trait GuardedTerminal {
    /// `abort_transaction`, ending the server session if cancelled mid-send.
    fn abort_guarded(&mut self) -> impl std::future::Future<Output = Result<()>> + Send;
    /// `commit_transaction`, ending the server session if cancelled mid-send.
    fn commit_guarded(&mut self) -> impl std::future::Future<Output = Result<()>> + Send;
}

#[allow(clippy::disallowed_methods)]
impl GuardedTerminal for ClientSession {
    async fn abort_guarded(&mut self) -> Result<()> {
        let guard = KillOnDrop::arm(self);
        let result = self.abort_transaction().await;
        guard.disarm();
        result
    }

    async fn commit_guarded(&mut self) -> Result<()> {
        let guard = KillOnDrop::arm(self);
        let result = self.commit_transaction().await;
        guard.disarm();
        result
    }
}

/// Ends `lsid`'s server session, aborting any transaction it still holds.
/// Users may end their own sessions without an additional privilege.
pub(crate) async fn kill_session(client: &Client, lsid: &Document) -> Result<()> {
    client
        .database("admin")
        .run_command(doc! { "killSessions": [lsid.clone()] })
        .await
        .map(|_| ())
}

struct KillOnDrop {
    armed: Option<(Client, Document)>,
}

impl KillOnDrop {
    fn arm(session: &ClientSession) -> Self {
        Self {
            armed: Some((session.client(), session.id().clone())),
        }
    }

    fn disarm(mut self) {
        self.armed = None;
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let Some((client, lsid)) = self.armed.take() else {
            return;
        };
        spawn_end_session(client, lsid, None);
    }
}

pub(crate) fn end_owned_session(session: ClientSession) {
    spawn_end_session(session.client(), session.id().clone(), Some(session));
}

fn spawn_end_session(client: Client, lsid: Document, held_session: Option<ClientSession>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("cancelled transaction has no runtime to end its session");
        return;
    };
    runtime.spawn(
        async move {
            match tokio::time::timeout(KILL_DEADLINE, kill_session(&client, &lsid)).await {
                Ok(Ok(())) => tracing::debug!("ended the session of a cancelled transaction"),
                Ok(Err(error)) => {
                    tracing::warn!(%error, "could not end a cancelled transaction session")
                }
                Err(_) => {
                    tracing::warn!("ending a cancelled transaction session exceeded its deadline")
                }
            }
            // Keep the session checked out through the bounded kill attempt.
            // After a timeout, an already-sent server command may still finish late.
            drop(held_session);
        }
        .in_current_span(),
    );
}

#[cfg(test)]
#[path = "transaction_terminal_tests.rs"]
mod tests;
