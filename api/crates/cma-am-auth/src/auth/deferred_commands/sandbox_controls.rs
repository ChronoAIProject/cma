use super::*;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::control::{
    SandboxControlAuthorization, SandboxControlAuthorizer, SandboxControlRecord,
};
use std::sync::Arc;

#[async_trait::async_trait]
impl SandboxControlAuthorizer for AuthState {
    async fn authorize_sandbox_control(
        &self,
        access: &ResourceAccessService,
        record: &SandboxControlRecord,
    ) -> Result<Arc<SandboxControlAuthorization>, ResourceCommandAuthorizationError> {
        tokio::time::timeout(VERIFICATION_TIMEOUT, async {
            record
                .validate()
                .map_err(|_| CommandAuthorizationError::Revoked)?;
            let envelope = &record.envelope;
            self.require_actor_owner_accounts(&envelope.actor, &envelope.owner)
                .await?;
            envelope
                .actor_subject()
                .map_err(|_| CommandAuthorizationError::Revoked)?;
            let state = self
                .command_actor_custody(&envelope.actor, &envelope.authorization)
                .await?;
            let request = self.verify_command_custody(&state).await?;
            let mut context = self.resource_request_context(&request);
            context.actor = envelope.actor.clone();
            let credential = request
                .delegated_credential
                .as_deref()
                .ok_or(CommandAuthorizationError::Revoked)?;
            let proof = access
                .authorize_sandbox_control(&context, credential, record)
                .await?;
            self.require_command_custody(&state).await?;
            proof.require_current()?;
            Ok(proof)
        })
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }
}
