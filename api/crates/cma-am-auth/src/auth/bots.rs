//! Resolve current, refreshable human authority for owner-admitted Bot work.
use super::{cookie_authorization, persistence::SecretValue, refresh, AuthState};
use layer2_core::{bots::BotError, Capability, SubjectId};
use std::time::{Duration, SystemTime};

impl AuthState {
    pub async fn bot_authority_token(
        &self,
        bot: &layer2_core::bots::Bot,
    ) -> Result<SecretValue, BotError> {
        use layer2_core::resource_access::{
            OrganizationAuthority, OrganizationRole, ResourceOwner,
        };
        let token = match (bot.authorization_grant_id(), bot.credential_incarnation()) {
            (Some(grant), Some(incarnation)) => {
                self.linked_bot_token(grant, bot.credential_subject(), incarnation)
                    .await?
            }
            (None, None) => self.bot_owner_token(bot.credential_subject()).await?,
            _ => return Err(BotError::Forbidden),
        };
        if let ResourceOwner::Organization { subject } = bot.resource_owner() {
            let caller = layer2_core::Caller::new(
                bot.credential_subject().clone(),
                layer2_core::Authorization::none(),
            );
            let context = self
                .organization_context(&caller, token.expose(), &subject)
                .await
                .map_err(|_| BotError::Forbidden)?;
            if !context.memberships.iter().any(|membership| {
                membership.organization == subject
                    && membership.role == OrganizationRole::Admin
                    && membership.is_current(&context.credential, SystemTime::now())
            }) {
                return Err(BotError::Forbidden);
            }
        }
        Ok(token)
    }
    /// An API key resolving to a human's subject cannot borrow that human's
    /// renewable login to mint broader keys. Admission uses the actual login.
    pub async fn authorize_bot_request(
        &self,
        request: &super::RequestAuth,
    ) -> Result<bool, layer2_core::StoreError> {
        // Proxy commands may use an owner's retained token for execution, but
        // that custody does not turn the proxy grant into personal Bot consent.
        if matches!(
            request.credential_source,
            layer2_core::resource_access::CredentialSource::IdentityProxy { .. }
        ) {
            return Ok(false);
        }
        let Some(credential) = request.delegated_credential.as_deref() else {
            return Ok(false);
        };
        if !personal(credential) {
            return Ok(false);
        }
        let Some(state) = self.store.subject(&request.subject).await? else {
            return Ok(false);
        };
        state.validate()?;
        Ok(state.subject == request.subject
            && state.tokens.as_ref().is_some_and(|tokens| {
                tokens.access_token.expose() == credential
                    && tokens.expires_at > SystemTime::now()
                    && tokens
                        .refresh_token
                        .as_ref()
                        .is_some_and(|t| !t.expose().is_empty())
            }))
    }
    pub async fn bot_owner_token(&self, owner: &SubjectId) -> Result<SecretValue, BotError> {
        self.bot_owner_token_fenced(owner, None).await
    }

    pub(crate) async fn bot_owner_token_fenced(
        &self,
        owner: &SubjectId,
        expected_incarnation: Option<&str>,
    ) -> Result<SecretValue, BotError> {
        self.renewable_actor_token(owner, expected_incarnation, Some(Capability::ManageOwnBots))
            .await
    }

    pub(in crate::auth) async fn actor_token_fenced(
        &self,
        owner: &SubjectId,
        expected_incarnation: Option<&str>,
    ) -> Result<SecretValue, BotError> {
        self.renewable_actor_token(owner, expected_incarnation, None)
            .await
    }

    async fn renewable_actor_token(
        &self,
        owner: &SubjectId,
        expected_incarnation: Option<&str>,
        required: Option<Capability>,
    ) -> Result<SecretValue, BotError> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let mut state = self
                .store
                .subject(owner)
                .await
                .map_err(|_| BotError::Unavailable)?
                .ok_or(BotError::Forbidden)?;
            state.validate().map_err(|_| BotError::Unavailable)?;
            if state.subject != *owner
                || expected_incarnation.is_some_and(|expected| expected != state.incarnation)
            {
                return Err(BotError::Forbidden);
            }
            let incarnation = state.incarnation.clone();
            refresh::refresh_if_needed(self, &mut state)
                .await
                .map_err(|_| BotError::Unavailable)?;
            if state.incarnation != incarnation {
                return Err(BotError::Forbidden);
            }
            let tokens = state.tokens.as_ref().ok_or(BotError::Forbidden)?;
            if tokens
                .refresh_token
                .as_ref()
                .is_none_or(|t| t.expose().is_empty())
                || tokens.expires_at <= SystemTime::now() + Duration::from_secs(60)
                || !personal(tokens.access_token.expose())
            {
                return Err(BotError::Forbidden);
            }
            let request = cookie_authorization::request_for_cookie(self, owner, state.clone())
                .await
                .map_err(|_| BotError::Unavailable)?;
            if required.is_some_and(|capability| !request.authorization.allows(capability))
                || request.delegated_credential.as_deref() != Some(tokens.access_token.expose())
            {
                return Err(BotError::Forbidden);
            }
            cookie_authorization::require_current_custody(self, &state)
                .await
                .map_err(|_| BotError::Forbidden)?;
            Ok(tokens.access_token.clone())
        })
        .await
        .map_err(|_| BotError::Unavailable)?
    }
}

fn personal(token: &str) -> bool {
    !token.starts_with("nyx_")
        && !token.starts_with("nyxid_")
        && super::bearer::permits_personal_authority(token)
}
