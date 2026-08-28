//! Test doubles for what a session shows and what it presents.
//!
//! Split from `test_support` so both stay within the project's file-size
//! limit, and re-exported from there so a test still has one import.

use crate::ids::SubjectId;
use crate::terminal::{ScreenModel, ScreenSnapshot, TerminalGeometry};

/// A screen that keeps everything it is given.
///
/// The null screen is right for tests about lifecycle, where the content is
/// irrelevant. It is exactly wrong for tests about whether content survives,
/// which would pass against a model that never held anything.
#[derive(Default)]
pub(crate) struct RecordingScreen {
    geometry: TerminalGeometry,
    bytes: Vec<u8>,
}

impl ScreenModel for RecordingScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    fn resize(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;
    }
    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.geometry,
            rows: Vec::new(),
        }
    }
    fn repaint_bytes(&self) -> Vec<u8> {
        self.bytes.clone()
    }
}

/// A credential holder that always has the same one.
pub(crate) struct FixedCredential(pub(crate) String);

#[async_trait::async_trait]
impl crate::store::CredentialSource for FixedCredential {
    async fn exact_services_available(
        &self,
        _subject: &SubjectId,
        _services: &[crate::environment::ServiceId],
    ) -> Result<bool, crate::store::StoreError> {
        // Test-only fixed sources stand in for an already admitted grant.
        Ok(true)
    }

    async fn credential_for(
        &self,
        _session: &crate::ids::SessionId,
        _subject: &SubjectId,
        _services: &[crate::environment::ServiceId],
        _caller_credential: Option<&crate::session::CallerCredential>,
    ) -> Result<Option<String>, crate::store::StoreError> {
        Ok(Some(self.0.clone()))
    }

    async fn release(
        &self,
        _session: &crate::ids::SessionId,
    ) -> Result<(), crate::store::StoreError> {
        Ok(())
    }
}
