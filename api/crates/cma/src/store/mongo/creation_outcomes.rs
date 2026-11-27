use std::sync::Arc;

use layer2_core::StoreError;

use super::MongoStores;
use crate::store::creation_outcomes::CreationOutcomes;

impl MongoStores {
    pub(crate) fn with_creation_writer_epoch(
        mut self,
        epoch: Option<&str>,
        terminal_settlement: bool,
    ) -> Self {
        if let Some(epoch) = epoch {
            Arc::make_mut(&mut self.sandbox_model).creation_writer_epoch = Some(epoch.to_owned());
            Arc::make_mut(&mut self.agent_lifecycle).creation_writer_epoch = Some(epoch.to_owned());
        }
        Arc::make_mut(&mut self.agent_lifecycle).creation_terminal_settlement_enabled =
            terminal_settlement;
        self
    }
    pub(crate) async fn creation_outcomes(
        &self,
        config: crate::config::CreationOutcomesConfig,
    ) -> Result<CreationOutcomes, StoreError> {
        CreationOutcomes::open(self.client.database(&self.database), config).await
    }
}
