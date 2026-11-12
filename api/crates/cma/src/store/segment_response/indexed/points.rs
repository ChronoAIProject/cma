use super::*;

impl Reader<'_> {
    pub(super) async fn source_points(
        &mut self,
        sequences: &[u64],
    ) -> Result<Vec<AgentLogEntry>, SandboxStoreError> {
        capture::points(
            self.store,
            &self.captured,
            sequences,
            &mut self.session,
            capture::CaptureMode::Snapshot,
        )
        .await
        .map_err(transaction)
    }
}
