//! Reconstruct command custody at the receiving AM; a peer's context is not authority.
use super::*;
use layer2_core::resource_access::{CallerContext, CredentialSource};
use std::sync::Arc;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerAdmissionIdentity {
    pub actor: AgentActor,
    pub authority: AgentAuthorizationRevision,
}

#[cfg(test)]
mod tests;

impl AuthState {
    pub(crate) async fn verify_owner_admission_origin(
        self: &Arc<Self>,
        context: &CallerContext,
        credential: &str,
        method: &axum::http::Method,
        uri: &axum::http::Uri,
        headers: &axum::http::HeaderMap,
    ) -> Result<(), CommandAuthorizationError> {
        if context.credential_source == CredentialSource::Binding {
            let request = self
                .resolve_request(headers)
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?
                .ok_or(CommandAuthorizationError::Revoked)?;
            let principal = request
                .binding
                .as_ref()
                .ok_or(CommandAuthorizationError::Revoked)?;
            if !crate::auth::binding::permits_request(method, uri.path(), principal.binding_id())
                || self.resource_request_context(&request) != *context
                || request.delegated_credential.as_deref() != Some(credential)
            {
                return Err(CommandAuthorizationError::Revoked);
            }
            return Ok(());
        }
        if matches!(
            context.credential_source,
            CredentialSource::Delegated { .. }
        ) {
            // A delegated actor may be selected by an authenticated binding
            // operation. Recheck that outer request's original route, then keep
            // its credential distinct from the independently retained grant.
            let request = self
                .resolve_request(headers)
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?
                .ok_or(CommandAuthorizationError::Revoked)?;
            if let Some(principal) = &request.binding {
                if !crate::auth::binding::permits_request(
                    method,
                    uri.path(),
                    principal.binding_id(),
                ) {
                    return Err(CommandAuthorizationError::Revoked);
                }
                return Ok(());
            }
            if request.subject != context.caller.subject
                || request.delegated_credential.as_deref() != Some(credential)
            {
                return Err(CommandAuthorizationError::Revoked);
            }
            let outer = self.resource_request_context(&request);
            crate::auth::relay::RelayCaller::verify(
                self.clone(),
                headers,
                &outer,
                Some(credential),
            )
            .await
            .map_err(|_| CommandAuthorizationError::Revoked)?
            .current()
            .await
            .map_err(|_| CommandAuthorizationError::Revoked)?;
            return Ok(());
        }
        crate::auth::relay::RelayCaller::verify(self.clone(), headers, context, Some(credential))
            .await
            .map_err(|status| match status {
                axum::http::StatusCode::SERVICE_UNAVAILABLE => {
                    CommandAuthorizationError::Unavailable
                }
                _ => CommandAuthorizationError::Revoked,
            })?
            .current()
            .await
            .map_err(|_| CommandAuthorizationError::Revoked)
    }

    pub(crate) async fn capture_owner_admission(
        &self,
        context: &CallerContext,
        credential: &str,
    ) -> Result<OwnerAdmissionIdentity, CommandAuthorizationError> {
        if !context.validate_principal() {
            return Err(CommandAuthorizationError::Revoked);
        }
        let owner = if context.credential_source == CredentialSource::Binding {
            self.binding_request(credential.to_owned())
                .await
                .map_err(|_| CommandAuthorizationError::Unavailable)?
                .and_then(|request| request.binding)
                .ok_or(CommandAuthorizationError::Revoked)?
                .resource_owner()
                .clone()
        } else {
            // Non-binding custody is selected by the actor, independently of
            // resource ownership. The command service checks resource policy.
            AgentOwner::Person {
                subject: context.caller.subject.clone(),
            }
        };
        let identity = OwnerAdmissionIdentity {
            actor: context.actor.clone(),
            authority: self
                .command_admission_authority(context, credential, &owner)
                .await?,
        };
        let fresh = self.restore_owner_admission(&identity, credential).await?;
        if fresh.caller != context.caller
            || fresh.credential != context.credential
            || fresh.credential_source != context.credential_source
            || fresh.actor != context.actor
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(identity)
    }

    pub(crate) async fn restore_owner_admission(
        &self,
        identity: &OwnerAdmissionIdentity,
        credential: &str,
    ) -> Result<CallerContext, CommandAuthorizationError> {
        tokio::time::timeout(VERIFICATION_TIMEOUT, async {
            if identity.authority.authority_id.starts_with("binding:") {
                let request = self
                    .binding_request(credential.to_owned())
                    .await
                    .map_err(|_| CommandAuthorizationError::Unavailable)?
                    .ok_or(CommandAuthorizationError::Revoked)?;
                let principal = request
                    .binding
                    .as_ref()
                    .ok_or(CommandAuthorizationError::Revoked)?;
                let context = self.resource_request_context(&request);
                if identity.actor != context.actor
                    || identity.authority != principal.command_authority()
                    || request.delegated_credential.as_deref() != Some(credential)
                {
                    return Err(CommandAuthorizationError::Revoked);
                }
                return Ok(context);
            }
            let custody = self
                .command_actor_custody(&identity.actor, &identity.authority)
                .await?;
            let request = self.verify_command_custody(&custody).await?;
            if request.delegated_credential.as_deref() != Some(credential) {
                // Refresh or credential replacement never substitutes a new
                // credential for the original in-flight admission.
                return Err(CommandAuthorizationError::Revoked);
            }
            let mut context = self.resource_request_context(&request);
            context.actor = identity.actor.clone();
            if !context.validate_principal() {
                return Err(CommandAuthorizationError::Revoked);
            }
            self.require_command_custody(&custody).await?;
            Ok(context)
        })
        .await
        .map_err(|_| CommandAuthorizationError::Unavailable)?
    }
}
