use crate::delivery::{
    destination::WebhookDestination, intents::DeliveryOutcome, local_stream::LocalStreamOutcome,
};
use cmaeg_core::{channel::Sink, seal::ReplyAddress};
use serde::{Deserialize, Serialize};

/// At most one local leg and MAX_SINKS webhook legs, inside the sealed frame record.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomCheckpoint {
    pub local: Option<LocalStreamOutcome>,
    pub webhooks: Vec<WebhookLeg>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookLeg {
    pub destination: WebhookDestination,
    pub status: WebhookStatus,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum WebhookStatus {
    NotStarted,
    Intent,
    Settled { outcome: DeliveryOutcome },
    Unknown,
}
impl CustomCheckpoint {
    pub(super) fn new(
        event: &cmaeg_core::seal::EventRefPayload,
        body: &super::FrozenTurn,
        terminal: bool,
    ) -> Self {
        Self {
            local: matches!(event.reply, ReplyAddress::Sync { .. })
                .then_some(LocalStreamOutcome::Pending),
            webhooks: body
                .delivery
                .policy
                .sinks
                .iter()
                .filter_map(|sink| match sink {
                    Sink::ResultWebhook { url, hmac_key_id } if terminal => Some(WebhookLeg {
                        destination: WebhookDestination {
                            url: url.clone(),
                            hmac_key_id: hmac_key_id.clone(),
                        },
                        status: WebhookStatus::NotStarted,
                    }),
                    _ => None,
                })
                .collect(),
        }
    }
    pub(super) fn valid_for(&self, expected: &Self) -> bool {
        self.local.is_some() == expected.local.is_some()
            && self.webhooks.len() <= cmaeg_core::channel::MAX_SINKS
            && self.webhooks.len() == expected.webhooks.len()
            && self
                .webhooks
                .iter()
                .zip(&expected.webhooks)
                .all(|(a, b)| a.destination == b.destination)
    }
}
