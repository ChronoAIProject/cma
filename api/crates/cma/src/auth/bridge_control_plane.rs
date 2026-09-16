//! Explicit Bridge grants select one existing encrypted OAuth sign-in. Refresh
//! remains owned by AuthState; a second copy of a rotating refresh token is never
//! created. A browser cookie is unnecessary after deliberate grant admission.
use std::time::{Duration, SystemTime};

use layer2_core::bridge::{BridgeError, BridgeSetupBlock};
use layer2_core::resource_access::{
    CallerContext, OrganizationAuthority, OrganizationRole, ResourceActor, ResourceOwner,
};
use layer2_core::{Caller, Capability, SubjectId};

use super::{cookie_authorization, persistence::SubjectAuthState, refresh, AuthState};

const IO_TIMEOUT: Duration = Duration::from_secs(15);

/// Opaque, short-lived authority for trusted control-plane adapters only.
pub(crate) struct BridgeHumanLease {
    state: SubjectAuthState,
    context: CallerContext,
    deadline: tokio::time::Instant,
}

impl BridgeHumanLease {
    #[cfg(test)]
    pub(crate) fn fixture(
        state: SubjectAuthState,
        context: CallerContext,
        deadline: tokio::time::Instant,
    ) -> Self {
        assert!(context.validate_principal());
        assert_eq!(state.subject, context.caller.subject);
        assert!(state.tokens.as_ref().is_some_and(|tokens| {
            !tokens.access_token.expose().is_empty() && tokens.refresh_token.is_some()
        }));
        Self {
            state,
            context,
            deadline,
        }
    }

    pub(crate) fn subject(&self) -> &SubjectId {
        &self.state.subject
    }
    pub(crate) fn incarnation(&self) -> &str {
        &self.state.incarnation
    }
    pub(crate) fn credential(&self) -> &str {
        self.state
            .tokens
            .as_ref()
            .expect("verified tokens")
            .access_token
            .expose()
    }
    pub(crate) fn deadline(&self) -> tokio::time::Instant {
        self.deadline
    }
    pub(crate) fn actor_context(&self) -> &CallerContext {
        &self.context
    }
}

impl AuthState {
    pub(crate) async fn capture_bridge_human(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &ResourceOwner,
    ) -> Result<BridgeHumanLease, BridgeError> {
        tokio::time::timeout(IO_TIMEOUT, async {
            if !context.validate_principal()
                || !matches!(
                    context.actor,
                    ResourceActor::Human { .. }
                        | ResourceActor::Api {
                            delegation_id: None,
                            ..
                        }
                )
                || self
                    .provider_credential_context(&context.caller.subject, credential)
                    .as_ref()
                    != Some(&context.credential)
            {
                return Err(reconnect());
            }
            let state = self.bridge_human_state(&context.caller.subject).await?;
            if !state.tokens.as_ref().is_some_and(|tokens| {
                tokens.access_token.expose() == credential && tokens.expires_at > SystemTime::now()
            }) {
                return Err(reconnect());
            }
            self.verify_bridge_human(state, owner).await
        })
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)?
    }

    pub(crate) async fn resolve_bridge_human(
        &self,
        subject: &SubjectId,
        incarnation: &str,
        owner: &ResourceOwner,
    ) -> Result<BridgeHumanLease, BridgeError> {
        tokio::time::timeout(IO_TIMEOUT, async {
            let mut state = self.bridge_human_state(subject).await?;
            if state.incarnation != incarnation {
                return Err(reconnect());
            }
            refresh::refresh_if_needed(self, &mut state)
                .await
                .map_err(|_| BridgeError::AuthorityUnavailable)?;
            if state.incarnation != incarnation {
                return Err(reconnect());
            }
            self.verify_bridge_human(state, owner).await
        })
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)?
    }

    async fn bridge_human_state(
        &self,
        subject: &SubjectId,
    ) -> Result<SubjectAuthState, BridgeError> {
        let state = self
            .store
            .subject(subject)
            .await
            .map_err(|_| BridgeError::AuthorityUnavailable)?
            .ok_or_else(reconnect)?;
        state
            .validate()
            .map_err(|_| BridgeError::AuthorityUnavailable)?;
        if state.subject != *subject {
            return Err(reconnect());
        }
        Ok(state)
    }

    async fn verify_bridge_human(
        &self,
        state: SubjectAuthState,
        owner: &ResourceOwner,
    ) -> Result<BridgeHumanLease, BridgeError> {
        let started = tokio::time::Instant::now();
        let now = SystemTime::now();
        let tokens = state.tokens.as_ref().ok_or_else(reconnect)?;
        if tokens
            .refresh_token
            .as_ref()
            .is_none_or(|token| token.expose().trim().is_empty())
            || tokens.access_token.expose().starts_with("nyx_")
            || tokens.access_token.expose().starts_with("nyxid_")
            || !super::bearer::permits_personal_authority(tokens.access_token.expose())
        {
            return Err(reconnect());
        }
        let remaining = tokens
            .expires_at
            .duration_since(now)
            .map_err(|_| reconnect())?;
        let mut deadline = started + remaining.min(Duration::from_secs(60));
        let request = cookie_authorization::request_for_cookie(self, &state.subject, state.clone())
            .await
            .map_err(|_| BridgeError::AuthorityUnavailable)?;
        if !request.authorization.allows(Capability::UseOwnSandboxes)
            || request.delegated_credential.as_deref() != Some(tokens.access_token.expose())
        {
            return Err(reconnect());
        }
        let mut context = self.resource_request_context(&request);
        match owner {
            ResourceOwner::Person { subject } if subject == &state.subject => {}
            ResourceOwner::Organization { subject } => {
                let caller = Caller::new(state.subject.clone(), request.authorization);
                context = self
                    .organization_context(&caller, tokens.access_token.expose(), subject)
                    .await
                    .map_err(|error| match error {
                        layer2_core::resource_access::MembershipError::Unavailable => {
                            BridgeError::AuthorityUnavailable
                        }
                        _ => reconnect(),
                    })?;
                let evidence = context
                    .memberships
                    .first()
                    .filter(|evidence| {
                        context.memberships.len() == 1
                            && evidence.organization == *subject
                            && evidence.role == OrganizationRole::Admin
                            && evidence.is_current(&context.credential, SystemTime::now())
                    })
                    .ok_or_else(reconnect)?;
                deadline = deadline.min(evidence.monotonic_deadline());
            }
            _ => return Err(reconnect()),
        }
        let lease = BridgeHumanLease {
            state,
            context,
            deadline,
        };
        self.revalidate_bridge_human(&lease).await?;
        Ok(lease)
    }

    pub(crate) async fn revalidate_bridge_human(
        &self,
        lease: &BridgeHumanLease,
    ) -> Result<(), BridgeError> {
        cookie_authorization::require_current_custody(self, &lease.state)
            .await
            .map_err(|_| reconnect())?;
        if tokio::time::Instant::now() >= lease.deadline {
            return Err(reconnect());
        }
        Ok(())
    }
}

fn reconnect() -> BridgeError {
    BridgeError::ConnectionRequired(BridgeSetupBlock::RenewableAuthorityUnavailable)
}
