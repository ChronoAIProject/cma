//! A linked user requests a gateway-owned connection; CMA retains only its receipt.
use super::*;
use crate::auth::persistence::SecretValue;
use sha2::{Digest, Sha256};

impl SetupAuthority {
    pub(super) async fn connection_stamp(&self, setup: &BotSetup) -> Result<String, BotError> {
        let grant = self
            .auth
            .current_actor_grant(
                setup.grant_id.as_deref().ok_or(BotError::Forbidden)?,
                &authorization::actor(&setup.actor),
            )
            .await?;
        layer2_core::profiles::content_digest(&(
            &grant.grant_id,
            grant.subject.as_str(),
            &grant.credential_incarnation,
        ))
        .map_err(|_| BotError::Unavailable)
    }

    pub(super) async fn connection_credentials(
        &self,
        setup: &BotSetup,
    ) -> Result<(SecretValue, SecretValue), BotError> {
        let pending = setup
            .pending_connection
            .as_ref()
            .ok_or(BotError::Conflict)?;
        if self.connection_stamp(setup).await? != pending.authority_stamp {
            return Err(BotError::Forbidden);
        }
        let grant = setup.grant_id.as_deref().ok_or(BotError::Forbidden)?;
        let owner = setup.owner.as_ref().ok_or(BotError::Forbidden)?;
        let actor = authorization::actor(&setup.actor);
        self.auth
            .owner_authority_for_grant(grant, &actor, owner)
            .await?;
        let (_, user) = self.auth.context_for_grant(grant, &actor, owner).await?;
        let binding = self
            .store
            .binding(&setup.actor.binding_id)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        if binding.binding_id != self.binding {
            return Err(BotError::Forbidden);
        }
        let material = self
            .custody
            .list(&binding.sandbox_id)
            .await
            .map_err(|_| BotError::Unavailable)?
            .into_iter()
            .find(|material| {
                material.owner == binding.owner
                    && format!("{:x}", Sha256::digest(material.value.expose().as_bytes()))
                        == binding.key_sha256
            })
            .ok_or(BotError::Forbidden)?;
        // The original admitted event fixes the sender, binding generation and source.
        self.admitted_event(
            &setup.actor,
            &pending.source_event_id,
            &pending.source_event_ref,
            material.value.expose(),
        )
        .await?;
        Ok((user, SecretValue::new(material.value.expose().into())))
    }
}

impl SetupDependencies {
    pub(super) async fn resolve_connection(
        &self,
        setup: &BotSetup,
    ) -> Result<SetupConnectionStatus, BotError> {
        let pending = setup
            .pending_connection
            .as_ref()
            .ok_or(BotError::Conflict)?;
        let owner = setup.owner.as_ref().ok_or(BotError::Forbidden)?;
        let (user, agent) = self.connection_credentials(setup).await?;
        // Read the caller-allocated identity first to recover a lost POST response.
        let id = pending
            .connection_ref
            .as_deref()
            .unwrap_or(&pending.record_id);
        let connection = match self.connections.get(user.expose(), owner, id).await {
            Ok(connection) => connection,
            Err(BotError::NotFound) if pending.connection_ref.is_none() => {
                self.connections
                    .create(
                        user.expose(),
                        owner,
                        &pending.record_id,
                        &pending.source_event_ref,
                        agent.expose(),
                        setup.draft.name.as_deref(),
                    )
                    .await?
            }
            Err(error) => return Err(error),
        };
        // Refresh/owner changes during upstream IO must not publish another authority's result.
        self.connection_credentials(setup).await?;
        Ok(SetupConnectionStatus {
            connection_ref: connection.connection_ref,
            status: connection.status,
            setup_url: connection.setup_url,
        })
    }

    pub(super) async fn cancel_connection(&self, setup: &BotSetup) -> Result<(), BotError> {
        let pending = setup
            .pending_connection
            .as_ref()
            .ok_or(BotError::Conflict)?;
        let (user, _) = self.connection_credentials(setup).await?;
        self.connections
            .cancel(
                user.expose(),
                setup.owner.as_ref().ok_or(BotError::Forbidden)?,
                pending
                    .connection_ref
                    .as_deref()
                    .unwrap_or(&pending.record_id),
            )
            .await
    }
}
