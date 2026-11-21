use std::sync::Mutex;

use layer3_adapter::resource_execution_protocol::{ProtocolError, ProtocolResult};

struct State {
    closed: bool,
    entered: bool,
    preparing: bool,
    deadline: u64,
}

/// This lock covers only the trusted local release, never preparation, disk,
/// output or network IO. Closing is irreversible for this execution identity.
pub(super) struct Gate(Mutex<State>);

pub(super) struct EnterPermit<'a> {
    _gate: &'a Gate,
}

impl Gate {
    pub(super) fn new(deadline: u64, closed: bool) -> Self {
        Self(Mutex::new(State {
            closed,
            entered: closed,
            preparing: false,
            deadline,
        }))
    }

    pub(super) fn close(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.closed = true;
        }
    }

    pub(super) fn expire(&self, tick: u64) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return true;
        };
        if tick >= state.deadline {
            state.closed = true;
        }
        state.closed
    }

    pub(super) fn prepare(&self, tick: u64) -> ProtocolResult<()> {
        let mut state = self.0.lock().map_err(|_| ProtocolError::Unknown)?;
        if state.closed || tick >= state.deadline {
            return Err(ProtocolError::Expired);
        }
        if state.preparing || state.entered {
            return Err(ProtocolError::Unknown);
        }
        state.preparing = true;
        Ok(())
    }

    pub(super) fn prepared(&self) {
        if let Ok(mut state) = self.0.lock() {
            state.preparing = false;
        }
    }

    pub(super) fn enter(
        &self,
        tick: u64,
        shared: bool,
        release: impl FnOnce(EnterPermit<'_>) -> ProtocolResult<()>,
    ) -> ProtocolResult<()> {
        let mut state = self.0.lock().map_err(|_| ProtocolError::Unknown)?;
        if state.closed || tick >= state.deadline {
            return Err(ProtocolError::Expired);
        }
        if state.preparing || state.entered {
            return Err(ProtocolError::Unknown);
        }
        // A failed release may already have crossed the native boundary. It
        // cannot be retried or classified as rejected-before-start.
        state.entered = true;
        if !shared {
            state.deadline = u64::MAX;
        }
        if let Err(error) = release(EnterPermit { _gate: self }) {
            state.closed = true;
            return Err(error);
        }
        Ok(())
    }

    pub(super) fn renew(&self, tick: u64, deadline: u64) -> ProtocolResult<()> {
        let mut state = self.0.lock().map_err(|_| ProtocolError::Unknown)?;
        if state.closed || tick >= state.deadline || tick >= deadline {
            state.closed = true;
            return Err(ProtocolError::Expired);
        }
        if !state.entered || deadline < state.deadline {
            return Err(ProtocolError::EvidenceMismatch);
        }
        state.deadline = deadline;
        Ok(())
    }

    pub(super) fn ready_to_observe_stop(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|state| state.closed && !state.preparing)
    }

    pub(super) fn never_entered(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|state| state.closed && !state.preparing && !state.entered)
    }
}
