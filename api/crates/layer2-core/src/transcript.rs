//! Keeping a session's terminal readable after the process holding it dies.
//!
//! A screen model lives in memory, so everything someone has read and typed
//! disappears with the process that was serving it. The sandbox survives —
//! sessions come back and keep working — but they come back blank, and a
//! terminal that has forgotten the conversation above the prompt is not the
//! session someone left.
//!
//! So the transcript is captured to the context store while the session runs
//! and replayed into a fresh screen when one is rebuilt. It is captured on a
//! sweep rather than on every byte: agent output arrives in a stream of small
//! chunks, and a blob store is the wrong shape for that. The cost of the sweep
//! is losing the last few seconds of a session nobody was watching, which is
//! the right thing to trade against writing a quarter of a megabyte per
//! keystroke.

use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::ids::SessionId;
use crate::service::SessionService;

impl SessionService {
    /// Capture every live session's transcript that has changed since the last
    /// capture.
    pub async fn capture_transcripts(&self) -> usize {
        if self.context.is_none() {
            return 0;
        }
        let ids: Vec<SessionId> = self.entries.read().await.keys().cloned().collect();
        let mut captured = 0;
        for id in ids {
            if self.capture_transcript(&id).await {
                captured += 1;
            }
        }
        captured
    }

    /// Capture one session's transcript, if it has anything new to say.
    ///
    /// Returns whether anything was written, which is what lets the sweep stay
    /// quiet over sessions nobody is using.
    pub(crate) async fn capture_transcript(&self, id: &SessionId) -> bool {
        let Some(context) = self.context.clone() else {
            return false;
        };

        let (bytes, previously) = {
            let entries = self.entries.read().await;
            let Some(entry) = entries.get(id) else {
                return false;
            };
            let bytes = entry.screen.lock().await.repaint_bytes();
            (bytes, entry.captured_len)
        };

        // The transcript only ever grows within a session, so its length is
        // enough to tell whether anything happened — and comparing lengths
        // avoids hashing a quarter-megabyte buffer every few seconds.
        if bytes.is_empty() || bytes.len() == previously {
            return false;
        }
        let len = bytes.len();

        let reference = match context.put(id, bytes).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(session_id = %id, error = %e, "cannot store the transcript");
                return false;
            }
        };

        let record = {
            let mut entries = self.entries.write().await;
            let Some(entry) = entries.get_mut(id) else {
                return false;
            };
            entry.captured_len = len;
            let changed = entry.context_ref.as_deref() != Some(reference.as_str());
            entry.context_ref = Some(reference.clone());
            changed
        };

        // The reference is what a later process finds the transcript by, so it
        // has to reach the record — but only when it actually changes, which
        // for a stable key is once.
        if record {
            self.flush(id).await;
        }
        true
    }

    /// Everything a session's terminal has shown, whether or not it is running.
    ///
    /// The one way to read a session that has been stopped. Its runtime is
    /// gone, so there is nothing to attach to, but the work is not gone and
    /// somebody stopping a session did not ask to lose sight of it.
    pub async fn session_transcript(
        &self,
        caller: &Caller,
        id: &SessionId,
    ) -> Result<Vec<u8>, CoreError> {
        caller.authorization.require(Capability::UseOwnSessions)?;
        let record = self.record(id).await?;
        if !record.session.is_owned_by(&caller.subject) {
            return Err(CoreError::Forbidden);
        }

        // A live session's screen is ahead of anything stored, since capture
        // runs on a sweep rather than on every byte.
        if let Some(entry) = self.entries.read().await.get(id) {
            return Ok(entry.screen.lock().await.repaint_bytes());
        }
        Ok(self
            .stored_transcript(record.context_ref.as_deref())
            .await
            .unwrap_or_default())
    }

    /// Read back a session's stored transcript, if it has one.
    pub(crate) async fn stored_transcript(&self, reference: Option<&str>) -> Option<Vec<u8>> {
        let context = self.context.as_ref()?;
        let reference = reference?;
        match context.get(reference).await {
            Ok(bytes) => bytes,
            Err(e) => {
                // A missing transcript is worth saying out loud but is not
                // worth refusing the session over: an empty screen is worse
                // than a full one and far better than no session at all.
                tracing::warn!(reference, error = %e, "cannot read the stored transcript");
                None
            }
        }
    }
}
