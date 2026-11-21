//! A scheduling claim and its materialized writes share one commit boundary.
use super::*;
use crate::store::transaction_scope as tx;

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
        tx::run(
            &self.client,
            (self, key, holder, collection, &updates),
            |&(store, key, holder, collection, updates), session| {
                Box::pin(async move {
                    store
                        .bulk_updates_in_session(collection, updates.clone(), Some(&mut *session))
                        .await?;
                    // Fence last on every attempt: takeover must conflict with the
                    // same CONTROL write even after an earlier transient failure.
                    store.fence_control_transaction(key, holder, session).await
                })
            },
        )
        .await
    }
}
