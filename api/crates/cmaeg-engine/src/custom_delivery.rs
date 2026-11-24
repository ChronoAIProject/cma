use crate::DeliveryOutcome;
use cmaeg_core::{channel::Sink, seal::ReplyAddress};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookDestination {
    pub url: String,
    pub hmac_key_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalStreamOutcome {
    /// Durable reservation; whether enqueue happened is not yet recorded.
    Pending,
    /// Only the bounded in-process queue accepted the frame, not the reader.
    Buffered,
    /// No live waiter accepted this attempt, including exhausted local capacity.
    Unavailable,
    /// The process lost the enqueue result. Never retransmitted as a reconnect.
    Lost,
}

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
    pub fn new(event: &cmaeg_core::seal::EventRefPayload, sinks: &[Sink], webhook: bool) -> Self {
        Self {
            local: matches!(event.reply, ReplyAddress::Sync { .. })
                .then_some(LocalStreamOutcome::Pending),
            webhooks: sinks
                .iter()
                .filter_map(|sink| match sink {
                    Sink::ResultWebhook { url, hmac_key_id } if webhook => Some(WebhookLeg {
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
    pub fn valid_for(&self, expected: &Self) -> bool {
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
