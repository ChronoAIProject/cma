//! A scheduling claim and its materialized writes share one commit boundary.
use super::*;

impl ReportingStore {
    pub(crate) async fn controlled_updates(
        &self,
        key: &str,
        holder: &str,
        collection: &str,
        updates: Vec<Document>,
    ) -> Result<()> {
        if updates.len() > 200 {
            return Err(MetricsError::Busy);
        }
        self.check_circuit(now())?;
        let mut session = self.client.start_session().await.map_err(unavailable)?;
        session.start_transaction().await.map_err(unavailable)?;
        let result = self
            .bulk_updates_in_session(collection, updates, Some(&mut session))
            .await;
        self.commit_controlled_write(key, holder, &mut session, result)
            .await
    }

    pub(super) async fn commit_controlled_write<T>(
        &self,
        key: &str,
        holder: &str,
        session: &mut mongodb::ClientSession,
        result: Result<T>,
    ) -> Result<T> {
        // Fence last: a slow write cannot commit just because the holder was
        // live before IO. Takeover conflicts with this same CONTROL write.
        let result = match result {
            Ok(value) => self
                .fence_control_transaction(key, holder, session)
                .await
                .map(|()| value),
            Err(error) => Err(error),
        };
        match result {
            Ok(value) => {
                // An uncertain commit stays an error; the next cycle can safely
                // repeat these stable-identity, monotonic materializations.
                session.commit_transaction().await.map_err(unavailable)?;
                Ok(value)
            }
            Err(error) => {
                // A failed fence must not retain locks while its successor runs.
                // Cancellation still uses the driver's uncommitted-session drop.
                match tokio::time::timeout(DB_DEADLINE, session.abort_transaction()).await {
                    Ok(Ok(())) => {}
                    Ok(Err(_)) => tracing::warn!("reporting publication abort unconfirmed"),
                    Err(_) => tracing::warn!("reporting publication abort timed out"),
                }
                Err(error)
            }
        }
    }
}
