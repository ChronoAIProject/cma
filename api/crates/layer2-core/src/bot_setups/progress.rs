use super::{
    helpers::{grant, owner},
    *,
};
use crate::bots::BotError;

impl BotSetupService {
    pub(super) async fn finish_or_view(
        &self,
        mut setup: BotSetup,
        notice: Option<String>,
    ) -> Result<SetupView, BotError> {
        if setup.step == SetupStep::ExternalBot && setup.pending_connection.is_some() {
            let pending = setup
                .pending_connection
                .as_ref()
                .ok_or(BotError::Conflict)?;
            if pending.plan_digest != setup.plan_digest()? {
                return Err(BotError::Conflict);
            }
            let current = self.dependencies.channel_connection(&setup).await?;
            let previous = setup.revision;
            let pending = setup
                .pending_connection
                .as_mut()
                .ok_or(BotError::Conflict)?;
            pending.connection_ref = Some(current.connection_ref.clone());
            pending.status = current.status.clone();
            pending.setup_url = current.setup_url;
            if current.status == "ready" {
                setup.connection_ref = Some(current.connection_ref);
                setup.external_bot_id = None;
                setup.pending_connection = None;
                setup.step = SetupStep::Admission;
            }
            setup.revision += 1;
            setup.updated_at = crate::now_rfc3339();
            self.store.replace(&setup, previous).await?;
        }
        if setup.step == SetupStep::Provisioning && setup.bot_id.is_none() {
            let result = self.dependencies.provision(&setup).await?;
            let previous = setup.revision;
            setup.bot_id = Some(result.bot_id);
            setup.revision += 1;
            setup.updated_at = crate::now_rfc3339();
            self.store.replace(&setup, previous).await?;
        }
        if setup.step == SetupStep::Provisioning && setup.bot_id.is_some() {
            let result = self.dependencies.result(&setup).await?;
            if result.state == "active" {
                let previous = setup.revision;
                setup.step = SetupStep::Complete;
                setup.revision += 1;
                setup.updated_at = crate::now_rfc3339();
                self.store.replace(&setup, previous).await?;
            }
        }
        self.view(&setup, notice, None).await
    }

    pub(super) async fn catalog(
        &self,
        setup: &BotSetup,
    ) -> Result<Vec<SetupServiceChoice>, BotError> {
        let mut services = self
            .dependencies
            .services(&setup.actor, grant(setup)?, owner(setup)?)
            .await?;
        for service in &mut services {
            service.selected = service.required
                || setup.selected_services.contains(&service.id)
                || setup
                    .configuration
                    .sandbox
                    .nyxid_services
                    .iter()
                    .any(|id| id.as_str() == service.id || id.as_str() == service.slug);
        }
        services.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(services)
    }
}
