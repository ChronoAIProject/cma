//! Driving a live session's terminal.
//!
//! Split from the service so that file is about session state and this one is
//! about the bytes and geometry flowing to a running runtime. Every method here
//! requires ownership: driving a terminal is acting, not observing, and a
//! privileged reader deliberately holds no capability that reaches this far.

use std::sync::Arc;

use layer3_adapter::{TermSize, TerminalInput};

use crate::authz::Caller;
use crate::error::CoreError;
use crate::event::EventKind;
use crate::ids::SessionId;
use crate::service::{append, SessionService};
use crate::terminal::{ScreenSnapshot, TerminalGeometry};

impl SessionService {
    /// Deliver input to the session's terminal.
    pub async fn send_input(
        &self,
        caller: &Caller,
        id: &SessionId,
        bytes: Vec<u8>,
    ) -> Result<(), CoreError> {
        self.authorize(caller, id).await?;
        self.forward_input(id, TerminalInput::Bytes(bytes)).await;
        Ok(())
    }
    /// Send an out-of-band signal to the session's runtime.
    pub async fn send_signal(
        &self,
        caller: &Caller,
        id: &SessionId,
        signal: String,
    ) -> Result<(), CoreError> {
        self.authorize(caller, id).await?;
        self.forward_input(id, TerminalInput::Signal(signal)).await;
        Ok(())
    }
    /// Change the session's terminal geometry.
    pub async fn resize_terminal(
        &self,
        caller: &Caller,
        id: &SessionId,
        geometry: TerminalGeometry,
    ) -> Result<(), CoreError> {
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
    /// Read the current screen without holding a stream.
    pub async fn screen_snapshot(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<ScreenSnapshot, CoreError> {
        let screen = {
            let entries = self.entries.read().await;
            let entry = entries.get(id).ok_or(CoreError::SessionNotFound)?;
            if !caller.may_read_owned_by(&entry.session.owner) {
                return Err(CoreError::Forbidden);
            }
            Arc::clone(&entry.screen)
        };
        let snapshot = screen.lock().await.snapshot();
        Ok(snapshot)
    }
}
