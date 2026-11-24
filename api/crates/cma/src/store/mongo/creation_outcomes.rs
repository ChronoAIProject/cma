use super::*;

impl MongoStores {
    pub(crate) fn with_creation_writer_epoch(mut self, epoch: Option<&str>) -> Self {
        if let Some(epoch) = epoch {
            Arc::make_mut(&mut self.sandbox_model).creation_writer_epoch = Some(epoch.to_owned());
            Arc::make_mut(&mut self.agent_lifecycle).creation_writer_epoch = Some(epoch.to_owned());
        }
        self
    }
    pub(crate) async fn creation_outcomes(
        &self,
        config: crate::config::CreationOutcomesConfig,
    ) -> Result<super::creation_outcomes::CreationOutcomes, StoreError> {
        super::creation_outcomes::CreationOutcomes::open(
            self.client.database(&self.database),
            config,
        )
        .await
    }
}
