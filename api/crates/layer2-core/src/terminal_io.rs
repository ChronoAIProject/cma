//! Driving a live session's terminal.
//!
//! Split from the service so that file is about session state and this one is
//! about the bytes and geometry flowing to a running runtime. Every method here
//! requires ownership: driving a terminal is acting, not observing, and a
//! privileged reader deliberately holds no capability that reaches this far.

use std::sync::Arc;

use layer3_adapter::{TermSize, TerminalInput};

use crate::authz::Caller;
use crate::capabilities::Capability;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::service::{append, SessionService};
use crate::terminal::{ScreenSnapshot, TerminalGeometry};

impl SessionService {
    /// Deliver input to the session's terminal.
    ///
    /// Refused where somebody is holding the terminal. Keystrokes injected
    /// into a live attachment land in the middle of whatever escape sequence
    /// that client is drawing, and neither side can tell afterwards which
    /// characters were whose — so the call fails and says what is holding it
    /// rather than corrupting a screen somebody is looking at.
    pub async fn send_input(
        &self,
        caller: &Caller,
        id: &SessionId,
        bytes: Vec<u8>,
    ) -> Result<(), CoreError> {
        self.may_drive_terminal(caller, id).await?;
        self.refuse_while_attached(id).await?;
        self.forward_input(id, TerminalInput::Bytes(bytes)).await;
        Ok(())
    }
    /// Send an out-of-band signal to the session's runtime.
    ///
    /// Allowed while somebody is attached, unlike input. A signal is not a
    /// keystroke: it does not land in the byte stream and cannot corrupt what
    /// is being drawn, and stopping a runaway command is exactly the thing an
    /// owner may need to do from elsewhere while a screen is held open.
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
    /// Refused while somebody is attached, for the reason input is: the
    /// attached client declared the geometry it is drawing for, and resizing
    /// underneath it reflows a screen it is halfway through painting.
    pub async fn resize_terminal(
        &self,
        caller: &Caller,
        id: &SessionId,
        geometry: TerminalGeometry,
    ) -> Result<(), CoreError> {
        self.may_drive_terminal(caller, id).await?;
        self.refuse_while_attached(id).await?;
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

    /// Refuse if a client is holding this session's terminal.
    async fn refuse_while_attached(&self, id: &SessionId) -> Result<(), CoreError> {
        let entries = self.entries.read().await;
        let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
        match entry.attached.is_some() {
            true => Err(CoreError::AlreadyAttached),
            false => Ok(()),
        }
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
