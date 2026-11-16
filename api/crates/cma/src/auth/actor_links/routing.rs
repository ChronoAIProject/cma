//! Read-only consent selection before owner routing.
use super::*;

impl AuthState {
    /// Look up existing consent for routing without creating or renewing a link.
    pub(crate) async fn existing_actor_grant(
        &self,
        actor: &EventActor,
    ) -> Result<Option<ActorGrant>, BotError> {
        let key = actor.key().map_err(|_| BotError::Forbidden)?;
        let Some(row) = self
            .store
            .actor_link_for_actor(&key)
            .await
            .map_err(|_| BotError::Unavailable)?
        else {
            return Ok(None);
        };
        row.validate().map_err(|_| BotError::Unavailable)?;
        if row.actor != *actor {
            return Err(BotError::Forbidden);
        }
        if !row.active || row.revoked {
            return Ok(None);
        }
        self.current_actor_grant(&row.link_id, actor)
            .await
            .map(Some)
    }
}
