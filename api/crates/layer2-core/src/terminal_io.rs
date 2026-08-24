//! Driving a live session's terminal.
//!
//! Split from the service so that file is about session state and this one is
//! about the bytes and geometry flowing to a running runtime. Every method here
//! requires ownership: driving a terminal is acting, not observing, and a
//! privileged reader deliberately holds no capability that reaches this far.

use std::sync::Arc;

use layer3_adapter::{TermSize, TerminalInput, TerminalOutput};
use tokio::sync::{broadcast, mpsc, watch, Mutex};

use crate::authz::Caller;
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::log::append;
use crate::service::SessionService;
use crate::terminal::{ScreenModel, ScreenSnapshot, TerminalGeometry};

/// Pump runtime output into the screen model and out to any live subscriber.
pub(crate) fn spawn_relay(
    service: Arc<SessionService>,
    id: SessionId,
    mut output: mpsc::Receiver<TerminalOutput>,
    screen: Arc<Mutex<Box<dyn ScreenModel>>>,
    live: broadcast::Sender<Vec<u8>>,
    ending: watch::Sender<Option<crate::lease::Detached>>,
) {
    tokio::spawn(async move {
        let reported = loop {
            match output.recv().await {
                Some(TerminalOutput::Bytes(chunk)) => {
                    let mut screen = screen.lock().await;
                    screen.apply(&chunk);
                    // A send with no subscribers is not an error: the session is
                    // simply detached, and the screen has absorbed the bytes.
                    // Keep the same guard through the send: an attachment takes
                    // this lock before subscribing and repainting, making the
                    // snapshot/live boundary indivisible.
                    let _ = live.send(chunk);
                }
                Some(TerminalOutput::Exited { code }) => {
                    break Some(crate::lease::Detached::ProcessExited { code });
                }
                // The link ended without a process report. Preserve that
                // distinction instead of manufacturing a successful exit.
                None => break None,
            }
        };

        if let Some(reported) = reported {
            // Recorded before closing the byte stream. A receiver observes all
            // queued output, then Closed, then reads this exact ending.
            ending.send_replace(Some(reported));
        }

        // Close only the stream belonging to this relay. An older relay can end
        // after a rotation has already installed its replacement under the same
        // session id.
        let mut entries = service.entries.write().await;
        if let Some(entry) = entries.get_mut(&id) {
            if entry
                .live
                .as_ref()
                .is_some_and(|current| current.same_channel(&live))
            {
                entry.input = None;
                entry.live = None;
            }
        }
        drop(entries);
        drop(live);
        tracing::info!(session_id = %id, "runtime output ended");
    });
}

impl SessionService {
    /// Deliver input to the session's terminal.
    ///
    /// Unguarded against a live attachment on purpose: the client holding one
    /// drives the terminal through this very call, so refusing here would
    /// refuse the attachment its own keystrokes. Whether a caller that is *not*
    /// the holder may write is a question about that caller, and is asked where
    /// the caller is known — see [`Self::terminal_is_held`].
    pub async fn send_input(
        &self,
        caller: &Caller,
        id: &SessionId,
        bytes: Vec<u8>,
    ) -> Result<(), CoreError> {
        self.may_drive_terminal(caller, id).await?;
        self.forward_input(id, TerminalInput::Bytes(bytes)).await;
        Ok(())
    }
    /// Send a named, bounded terminal control request to the runtime.
    ///
    /// Allowed while somebody is attached, unlike free-form input. The runtime
    /// translates a documented name into one control byte, which is the bounded
    /// action an owner may need from elsewhere to stop a runaway command while
    /// a screen is held open.
    pub async fn send_signal(
        &self,
        caller: &Caller,
        id: &SessionId,
        signal: String,
    ) -> Result<(), CoreError> {
        self.may_drive_terminal(caller, id).await?;
        self.forward_input(id, TerminalInput::Signal(signal)).await;
        Ok(())
    }
    /// Change the session's terminal geometry.
    ///
    /// Unguarded here for the same reason as input: the attached client
    /// declares its own geometry through this call.
    pub async fn resize_terminal(
        &self,
        caller: &Caller,
        id: &SessionId,
        geometry: TerminalGeometry,
    ) -> Result<(), CoreError> {
        self.may_drive_terminal(caller, id).await?;
        let screen = {
            let mut entries = self.entries.write().await;
            let entry = entries.get_mut(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            entry.session.geometry = geometry;
            append(
                entry,
                EventKind::TerminalResized {
                    columns: geometry.columns(),
                    rows: geometry.rows(),
                },
            );
            Arc::clone(&entry.screen)
        };
        self.flush(id).await;
        screen.lock().await.resize(geometry);
        self.forward_input(
            id,
            TerminalInput::Resize(TermSize {
                cols: geometry.columns(),
                rows: geometry.rows(),
            }),
        )
        .await;
        Ok(())
    }
    /// Whether this caller may drive this session's terminal at all.
    ///
    /// Two questions that used to be one. Ownership says who may act; the
    /// capability says whether there is a terminal to act on. A conversation
    /// session has none, and writing keystrokes at it would be accepted and
    /// dropped — which is the shape of failure the capability model exists to
    /// remove.
    async fn may_drive_terminal(&self, caller: &Caller, id: &SessionId) -> Result<(), CoreError> {
        self.authorize(caller, id).await?;
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        entry.session.capabilities.require(Capability::Terminal)
    }

    /// Whether a client is currently holding this session's terminal.
    ///
    /// Asked by an interface that is about to write on behalf of somebody who
    /// is *not* that client. Keystrokes from elsewhere land in the middle of
    /// whatever escape sequence the holder is drawing, and afterwards neither
    /// side can tell which characters were whose.
    ///
    /// A question rather than a refusal, because the answer is not the same for
    /// every caller: the holder drives the terminal through the same calls, and
    /// a guard buried in them would refuse the attachment its own input.
    pub async fn terminal_is_held(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<bool, CoreError> {
        self.authorize(caller, id).await?;
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        Ok(entry.attached.is_some())
    }

    /// Read the current screen without holding a stream.
    ///
    /// Owner-only, on the same terms as reading the transcript: this is the
    /// same content by another route. A session's screen carries whatever the
    /// agent has printed, which includes the values of the environment bundle
    /// it was started with — so a capability that opens it is a capability
    /// that reads other people's secrets, and no operator holds one.
    pub async fn screen_snapshot(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<ScreenSnapshot, CoreError> {
        let screen = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
            if !entry.session.is_owned_by(&caller.subject) {
                return Err(CoreError::Forbidden);
            }
            Arc::clone(&entry.screen)
        };
        let snapshot = screen.lock().await.snapshot();
        Ok(snapshot)
    }
}
