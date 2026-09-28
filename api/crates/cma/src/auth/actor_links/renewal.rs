//! Keep active user grants renewable without retaining authority after revocation.
use super::*;
impl AuthState {
    pub(crate) fn spawn_actor_link_renewal(
        self: Arc<Self>,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    _ = interval.tick() => {}
                }
                let work = async {
                    let rows = self
                        .store
                        .due_actor_links(now().saturating_sub(RENEW_INTERVAL), 100)
                        .await?;
                    for mut row in rows {
                        row.validate()?;
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
                            Err(_) => return Err(unavailable()),
                        }
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
