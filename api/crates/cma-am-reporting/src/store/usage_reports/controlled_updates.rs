//! A scheduling claim and its materialized writes share one commit boundary.
use crate::{pool::DB_DEADLINE, transaction_scope as tx, ReportingStore};
use layer2_core::sandbox_metrics::{MetricsError, Result};
use mongodb::bson::Document;

impl ReportingStore {
    pub async fn controlled_updates(
        &self,
        key: &str,
        holder: &str,
        collection: &str,
        updates: Vec<Document>,
    ) -> Result<()> {
        if updates.len() > 200 {
            return Err(MetricsError::Busy);
        }
        tx::run(
            &self.client,
            (self, key, holder, collection, &updates),
            |&(store, key, holder, collection, updates), session| {
                Box::pin(async move {
                    store
                        .bulk_updates_with_deadline(
                            collection,
                            updates.clone(),
                            Some(&mut *session),
                            DB_DEADLINE,
                        )
                        .await?;
                    // Fence last on every attempt: takeover must conflict with the
                    // same CONTROL write even after an earlier transient failure.
                    store.fence_control_transaction(key, holder, session).await
                })
            },
            self.transaction_policy,
        )
        .await
    }
}
