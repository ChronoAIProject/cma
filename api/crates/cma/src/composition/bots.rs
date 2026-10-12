use super::*;
use layer2_core::bots::{BotControl, BotService, BotWorker};

pub(crate) struct BotComposition {
    pub(super) service: Arc<BotService>,
    worker: BotWorker,
    activity: Arc<ActivityService>,
}
impl BotComposition {
    pub(super) fn build(
        config: &Config,
        stores: &MongoStores,
        auth: Arc<AuthState>,
        profiles: Arc<layer2_core::event_workers::EventWorkerProfileService>,
    ) -> Result<(Router, Option<Self>), String> {
        let Some(gateway) = &config.gateway else {
            return Ok((Router::new(), None));
        };
        let AuthMode::Provider { base_url, .. } = &config.auth_mode else {
            return Err("CMAEG bots require NyxID authentication".into());
        };
        let external =
            crate::gateway::GatewayBots::new(gateway, auth, base_url, &config.deployment_id)?;
        let service = BotService::new(stores.bots.clone(), profiles, "cma_codex".into());
        let router = rest::bot_router(BotControl::new(service.clone(), external.clone()));
        let worker = BotWorker::new(
            service.clone(),
            external,
            format!("{}:bots:{}", config.deployment_id, uuid::Uuid::new_v4()),
        );
        Ok((
            router,
            Some(Self {
                service,
                worker,
                activity: ActivityService::new(stores.activity.clone()),
            }),
        ))
    }
    pub(crate) fn spawn(
        self,
        shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut after = None;
            let mut ticks = tokio::time::interval(std::time::Duration::from_secs(1));
            ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! { _ = crate::shutdown::signalled(shutdown.clone()) => break, _ = ticks.tick() => {} }
                let pass = async {
                    let rows = self.service.pending(after.as_deref(), 100).await?;
                    after = if rows.len() == 100 {
                        rows.last().map(|b| b.bot_id.clone())
                    } else {
                        None
                    };
                    for row in rows {
                        if !due(&row) {
                            continue;
                        }
                        let outcome = if let Err(error) = self.worker.run_due(&row.bot_id).await {
                            // Only bounded error enums and internal Bot IDs; no opaque references.
                            tracing::warn!(bot_id = %row.bot_id, %error, "bot work will retry");
                            layer2_core::ActivityOutcome::Failed("bot_work_retry".into())
                        } else {
                            layer2_core::ActivityOutcome::Succeeded
                        };
                        let mut record = layer2_core::ActivityRecord::new(
                            layer2_core::ActivityAction::Update,
                            outcome,
                        )
                        .on(layer2_core::ActivityTarget::Resource(row.bot_id));
                        record.actor = Some(row.owner);
                        if self.activity.record(&record).await.is_err() {
                            tracing::warn!("bot work activity could not be persisted");
                        }
                    }
                    Ok::<_, layer2_core::bots::BotError>(())
                };
                tokio::select! {
                    _ = crate::shutdown::signalled(shutdown.clone()) => break,
                    result = pass => { if result.is_err() { tracing::warn!("bot work discovery will retry"); } }
                }
            }
        })
    }
}

fn due(bot: &layer2_core::bots::Bot) -> bool {
    let now = layer2_core::seconds_from_rfc3339(&layer2_core::now_rfc3339()).unwrap_or_default();
    if let Some(work) = &bot.work {
        work.expires_at
            .as_deref()
            .and_then(layer2_core::seconds_from_rfc3339)
            .is_none_or(|expiry| expiry <= now)
    } else {
        bot.resources
            .credentials
            .expires_at
            .as_deref()
            .and_then(layer2_core::seconds_from_rfc3339)
            .is_some_and(|expiry| expiry <= now.saturating_add(3 * 86400))
    }
}
