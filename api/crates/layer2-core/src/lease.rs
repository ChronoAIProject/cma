//! The exclusive terminal attachment, and what releasing one means.
//!
//! Split from the service so that file is about the session core and this one
//! about the single piece of it a caller holds. Attachment is not a tenth core
//! operation — it is an exclusivity mode on the live stream — and holding this
//! is what makes it exclusive.
//!
//! Releasing is deliberately not a method. The lease ends when the holder stops
//! existing, which is the only moment that is true for a client that crashed as
//! well as one that closed politely.

use std::sync::Arc;

use tokio::sync::broadcast;

use crate::ids::SessionId;
use crate::service::SessionService;
use crate::terminal::TerminalGeometry;

/// An exclusive terminal attachment.
///
/// Holding this is what makes an attachment exclusive; dropping it releases the
/// lease. Attachment is not a tenth core operation — it is an exclusivity mode
/// on the live stream.
pub struct AttachmentHandle {
    pub(crate) session_id: SessionId,
    pub(crate) service: Arc<SessionService>,
    /// Which attachment this is, among those this session has issued.
    ///
    /// Carried so releasing is not a blunt instrument. A superseded holder
    /// drops its handle after its replacement is already live, and a release
    /// that did not check would detach the new client instead of the old one.
    pub(crate) generation: u64,
    /// Live terminal output for as long as the attachment is held.
    pub output: broadcast::Receiver<Vec<u8>>,
    /// Fires once when the owner has handed this attachment to somebody else.
    ///
    /// The superseded client is told rather than simply going quiet. A terminal
    /// that stops updating with no explanation is indistinguishable from a
    /// broken one, and the person looking at it will reload and take the
    /// attachment back — which is a fight nobody wins.
    pub superseded: broadcast::Receiver<()>,
    /// Bytes that repaint the current screen, delivered before live output.
    pub repaint: Vec<u8>,
    /// Geometry the repaint was rendered at.
    pub geometry: TerminalGeometry,
}

impl Drop for AttachmentHandle {
    fn drop(&mut self) {
        let service = Arc::clone(&self.service);
        let id = self.session_id.clone();
        let generation = self.generation;
        // Release synchronously where possible; the lock is never held across
        // an await inside the service, so a blocking release cannot deadlock.
        tokio::spawn(async move {
            service.release_attachment(&id, generation).await;
        });
    }
}
