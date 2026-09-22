use super::*;
use crate::{Caller, Capability};
use std::sync::Arc;

pub struct BotControl {
    pub service: Arc<BotService>,
    external: Arc<dyn BotExternal>,
}
impl BotControl {
    pub fn new(service: Arc<BotService>, external: Arc<dyn BotExternal>) -> Arc<Self> {
        Arc::new(Self { service, external })
    }
    pub async fn available(&self, caller: &Caller) -> Result<Vec<ExternalBot>, BotError> {
        caller
            .require(Capability::ManageOwnBots)
            .map_err(|_| BotError::Forbidden)?;
        self.external.available(&caller.subject).await
    }
    pub async fn test(&self, caller: &Caller, id: &str) -> Result<BotTestResult, BotError> {
        let bot = self.service.get(caller, id).await?;
        if bot.work.is_some() || !matches!(bot.state, BotState::Active | BotState::Disabled) {
            return Err(BotError::Conflict);
        }
        let result = self.external.test(&bot).await?;
        self.service.record_test(caller, id, result.clone()).await?;
        Ok(result)
    }
}
