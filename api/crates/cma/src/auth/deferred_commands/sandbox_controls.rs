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
            let subject = envelope
                .actor_subject()
                .map_err(|_| CommandAuthorizationError::Revoked)?;
            let state = self
                .retained_actor_state(subject, &envelope.authorization)
                .await?;
            let request = self.verify_human_state(state.clone()).await?;
            let mut context = self.resource_request_context(&request);
            context.actor = envelope.actor.clone();
            let credential = request
                .delegated_credential
                .as_deref()
                .ok_or(CommandAuthorizationError::Revoked)?;
            let proof = access
                .authorize_sandbox_control(&context, credential, record)
                .await?;
            cookie_authorization::require_current_custody(self, &state)
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?;
            proof.require_current()?;
            Ok(proof)
        })
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }
}
