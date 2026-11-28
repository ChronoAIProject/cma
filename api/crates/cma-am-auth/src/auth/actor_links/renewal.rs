//! Keep active user grants renewable without retaining authority after revocation.
use super::*;
impl AuthState {
    // Erasure needs the same grant and incarnation fencing without the standing
    // gate that intentionally disables ordinary actor operations and renewal.
    pub(super) async fn retained_link_token(
        &self,
        row: &ActorLinkRecord,
    ) -> Result<SecretValue, BotError> {
        let grant = row.grant()?;
        let token = self
            .actor_token_fenced(&grant.subject, Some(&grant.credential_incarnation))
            .await?;
        let latest = self.link_record(&grant.grant_id).await?;
        if !latest.same_active_authority(row) {
            return Err(BotError::Forbidden);
        }
        Ok(token)
    }

    pub fn spawn_actor_link_renewal(
        self: Arc<Self>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut after_actor_key: Option<String> = None;
            loop {
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    _ = interval.tick() => {}
                }
                let work = async {
                    let rows = self
                        .store
                        .due_actor_links(
                            now().saturating_sub(RENEW_INTERVAL),
                            after_actor_key.as_deref(),
                            100,
                        )
                        .await?;
                    if rows.is_empty() {
                        after_actor_key = None;
                    }
                    for mut row in rows {
                        row.validate()?;
                        let actor_key = row.actor_key.clone();
                        match self.link_token(&row).await {
                            Ok(_) => {
                                let revision = row.revision;
                                row.renewed_at = now();
                                row.revision += 1;
                                self.store.replace_actor_link(Some(revision), row).await?;
                            }
                            Err(BotError::Forbidden) => {
                                let _ = self.revoke_actor_link(&row.actor, &row.link_id).await;
                            }
                            Err(BotError::AccountDeletionInProgress) => {}
                            Err(_) => return Err(unavailable()),
                        }
                        after_actor_key = Some(actor_key);
                    }
                    Ok::<_, StoreError>(())
                };
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    result = work => if result.is_err() { tracing::warn!("actor link renewal will retry"); }
                }
            }
        })
    }
}
