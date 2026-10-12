//! Connection choices and cancellation for the generic channel setup step.
use super::{
    helpers::{grant, invalid_action, owner},
    BotSetup, BotSetupService, SetupConnectionRequest, SetupEvent, SetupStep,
};
use crate::bots::BotError;

impl BotSetupService {
    pub(super) async fn choose_external_connection(
        &self,
        setup: &mut BotSetup,
        event: &SetupEvent,
        action: &str,
    ) -> Result<(), BotError> {
        if action == "register" {
            if setup
                .pending_connection
                .as_ref()
                .is_some_and(SetupConnectionRequest::terminal)
            {
                self.abandon_connection(setup).await?;
            }
            if setup.pending_connection.is_none() {
                setup.pending_connection = Some(SetupConnectionRequest {
                    record_id: uuid::Uuid::new_v4().to_string(),
                    source_event_id: event.event_id.clone(),
                    source_event_ref: event.reference.clone(),
                    authority_stamp: self.dependencies.connection_authority_stamp(setup).await?,
                    plan_digest: setup.plan_digest()?,
                    connection_ref: None,
                    status: "pending".into(),
                    setup_url: None,
                });
            }
        } else if action != "refresh" {
            let id = action.strip_prefix("bot:").ok_or_else(invalid_action)?;
            let bots = self
                .dependencies
                .external_bots(&setup.actor, grant(setup)?, owner(setup)?)
                .await?;
            let selected = bots
                .iter()
                .find(|bot| bot.id == id)
                .ok_or(BotError::Forbidden)?;
            self.abandon_connection(setup).await?;
            setup.connection_ref = selected.connection_ref.clone();
            setup.external_bot_id = selected.connection_ref.is_none().then(|| id.into());
            setup.step = SetupStep::Admission;
        }
        Ok(())
    }
}

impl BotSetupService {
    pub(super) async fn abandon_connection(&self, setup: &mut BotSetup) -> Result<(), BotError> {
        if setup.pending_connection.is_some() {
            match self.dependencies.cancel_channel_connection(setup).await {
                Ok(()) | Err(BotError::Forbidden | BotError::NotFound) => {}
                Err(error) => return Err(error),
            }
            // Revoked authority cannot make upstream writes. Clearing the local
            // receipt still prevents any later completion from attaching here.
            setup.pending_connection = None;
        }
        Ok(())
    }
}
