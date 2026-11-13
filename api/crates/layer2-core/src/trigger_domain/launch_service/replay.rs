//! Reconcile durable Launch commands before admitting new work.
use super::{
    store_error, LaunchCreateResult, LaunchService, LaunchServiceError, PublicTriggerId, SubjectId,
};

impl LaunchService {
    pub(super) async fn replayed_command(
        &self,
        owner: &SubjectId,
        public_id: &PublicTriggerId,
        command_key_hash: &str,
        command_fingerprint: &str,
    ) -> Result<Option<LaunchCreateResult>, LaunchServiceError> {
        let Some(existing) = self
            .launches
            .launch_by_command(owner, public_id, command_key_hash)
            .await
            .map_err(store_error)?
        else {
            return Ok(None);
        };
        if existing.command_fingerprint != command_fingerprint {
            return Err(LaunchServiceError::IdempotencyKeyReused);
        }
        Ok(Some(LaunchCreateResult {
            launch: existing,
            created: false,
        }))
    }
}
