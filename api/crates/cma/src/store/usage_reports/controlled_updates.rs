//! Apply the Agent Manager pool circuit policy to persisted worker updates.
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
        self.persistence
            .controlled_updates(key, holder, collection, updates)
            .await
    }
}
