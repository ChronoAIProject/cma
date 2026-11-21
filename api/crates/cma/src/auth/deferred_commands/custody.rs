use super::*;
use crate::auth::persistence::ApiBearerCustody;

pub(super) enum CommandCustody {
    Personal(SubjectAuthState),
    Api(ApiBearerCustody),
    Delegated(Box<super::delegated::DelegatedCommandCustody>),
    Proxy(Box<crate::auth::nyxid_proxy::commands::ProxyCustody>),
}

impl AuthState {
    pub(super) async fn command_actor_custody(
        &self,
        actor: &AgentActor,
        authority: &AgentAuthorizationRevision,
    ) -> Result<CommandCustody, CommandAuthorizationError> {
        if let Some(subject) = actor.ordinary_subject() {
            self.require_command_account(subject).await?;
        }
        if let AgentActor::Api {
            subject,
            delegation_id: Some(grant_id),
        } = actor
        {
            return self
                .retained_delegated_custody(subject, grant_id, authority)
                .await
                .map(|custody| CommandCustody::Delegated(Box::new(custody)));
        }
        let subject = match actor {
            AgentActor::Api {
                subject,
                delegation_id: None,
            }
            | AgentActor::Human { subject } => subject,
            _ => return Err(CommandAuthorizationError::Revoked),
        };
        if authority.authority_id.starts_with("nyxid-proxy:") {
            if !matches!(
                actor,
                AgentActor::Api {
                    delegation_id: None,
                    ..
                }
            ) {
                return Err(CommandAuthorizationError::Revoked);
            }
            self.retained_proxy_custody(subject, authority)
                .await
                .map(|custody| CommandCustody::Proxy(Box::new(custody)))
        } else if authority.authority_id.starts_with("api:") {
            if !matches!(
                actor,
                AgentActor::Api {
                    delegation_id: None,
                    ..
                }
            ) {
                return Err(CommandAuthorizationError::Revoked);
            }
            self.retained_api_custody(subject, authority)
                .await
                .map(CommandCustody::Api)
        } else {
            self.retained_actor_state(subject, authority)
                .await
                .map(CommandCustody::Personal)
        }
    }

    pub(super) async fn verify_command_custody(
        &self,
        custody: &CommandCustody,
    ) -> Result<RequestAuth, CommandAuthorizationError> {
        match custody {
            CommandCustody::Personal(state) => self.verify_human_state(state.clone()).await,
            CommandCustody::Api(record) => self.verify_api_custody(record).await,
            CommandCustody::Delegated(custody) => self.verify_delegated_custody(custody).await,
            CommandCustody::Proxy(custody) => self.verify_proxy_custody(custody).await,
        }
    }

    pub(super) async fn require_command_custody(
        &self,
        custody: &CommandCustody,
    ) -> Result<(), CommandAuthorizationError> {
        match custody {
            CommandCustody::Personal(state) => {
                cookie_authorization::require_current_custody(self, state)
                    .await
                    .map_err(|_| CommandAuthorizationError::Unavailable)
            }
            CommandCustody::Api(record) => self.require_api_custody(record).await,
            CommandCustody::Delegated(custody) => self.require_delegated_custody(custody).await,
            CommandCustody::Proxy(custody) => self.require_proxy_custody(custody).await,
        }
    }
}
