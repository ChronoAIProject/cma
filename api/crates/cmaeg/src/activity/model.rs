use cmaeg_core::activity::{ActivityConversation, Actor, SourceKind};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

pub const LIMIT: usize = 50;
pub const RETENTION_DAYS: i64 = 7;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ActivityPage {
    pub items: Vec<ChannelActivity>,
    /// Opaque channel-bound snapshot watermark, usable as Last-Event-ID.
    pub cursor: String,
    pub retention_days: u8,
    pub limit: u16,
    pub has_more: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ChannelActivity {
    pub event_id: String,
    /// Payload category only; message text and event data are never copied here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// CloudEvent type, when this receipt represents an event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_type: Option<String>,
    /// Explicit CloudEvent time; absent when the source did not report one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_occurred_at: Option<String>,
    pub source: SourceKind,
    pub actor: Actor,
    pub conversation: ActivityConversation,
    /// Normalized event time: source timestamp when supplied, otherwise gateway normalization time.
    pub occurred_at: String,
    /// Gateway receive timestamp, persisted before provider launch.
    pub received_at: String,
    pub consumers: Vec<ActivityConsumer>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerState {
    Received,
    Queued,
    Dispatching,
    Accepted,
    Produced,
    Completed,
    Refused,
    Failed,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ActivityConsumer {
    pub consumer_id: String,
    pub display_name: Option<String>,
    pub agent_id: Option<String>,
    pub provider: Option<String>,
    pub conversation_id: Option<String>,
    pub state: ConsumerState,
    pub dispatched_at: Option<String>,
    pub accepted_at: Option<String>,
    pub produced_at: Option<String>,
    pub completed_at: Option<String>,
    pub response_id: Option<String>,
    pub deliveries: Vec<ActivityDelivery>,
}
impl ActivityConsumer {
    pub(crate) fn new(id: &str) -> Self {
        Self {
            consumer_id: id.into(),
            display_name: None,
            agent_id: None,
            provider: None,
            conversation_id: None,
            state: ConsumerState::Received,
            dispatched_at: None,
            accepted_at: None,
            produced_at: None,
            completed_at: None,
            response_id: None,
            deliveries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ActivityDelivery {
    pub step: String,
    pub destination: String,
    pub outcome: String,
    /// When the gateway observed the actual transport outcome.
    pub observed_at: String,
    /// Present only for a confirmed platform/transport receipt.
    pub confirmed_at: Option<String>,
    pub platform_message_id: Option<String>,
    pub recipients: Vec<ActivityRecipient>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct ActivityRecipient {
    pub kind: String,
    pub id: String,
    pub thread_id: Option<String>,
}

#[derive(Clone)]
pub struct Change {
    pub consumer: String,
    pub state: Option<ConsumerState>,
    pub at: String,
    pub response_id: Option<String>,
    pub delivery: Option<ActivityDelivery>,
    pub display_name: Option<String>,
    pub provider: Option<String>,
    pub conversation_id: Option<String>,
    pub agent_id: Option<String>,
    pub processing_completed: bool,
}
impl Change {
    pub fn state(consumer: &str, state: ConsumerState, response_id: Option<String>) -> Self {
        Self {
            consumer: consumer.into(),
            state: Some(state),
            at: super::now(),
            response_id,
            delivery: None,
            display_name: None,
            provider: None,
            conversation_id: None,
            agent_id: None,
            processing_completed: false,
        }
    }
    /// Processing completion does not prove a successful provider response.
    pub fn settled(consumer: &str, response_id: String) -> Self {
        let mut change = Self::state(consumer, ConsumerState::Unknown, Some(response_id));
        change.state = None;
        change.processing_completed = true;
        change
    }
    pub fn apply(&self, row: &mut ChannelActivity) {
        if !row.consumers.iter().any(|c| c.consumer_id == self.consumer) {
            if row.consumers.len() >= 16 {
                return;
            }
            row.consumers.push(ActivityConsumer::new(&self.consumer));
        }
        let consumer = row
            .consumers
            .iter_mut()
            .find(|c| c.consumer_id == self.consumer)
            .unwrap();
        if let Some(state) = &self.state {
            let timestamp = match state {
                ConsumerState::Dispatching => Some(&mut consumer.dispatched_at),
                ConsumerState::Accepted => Some(&mut consumer.accepted_at),
                ConsumerState::Produced => Some(&mut consumer.produced_at),
                ConsumerState::Completed | ConsumerState::Failed => {
                    Some(&mut consumer.completed_at)
                }
                _ => None,
            };
            if let Some(value) = timestamp {
                value.get_or_insert_with(|| self.at.clone());
            }
            // Replayed admission and accepted observations must not regress an
            // already produced/completed response's presentation.
            let rank = |s: &ConsumerState| match s {
                ConsumerState::Received => 0,
                ConsumerState::Queued => 1,
                ConsumerState::Dispatching => 2,
                ConsumerState::Accepted => 3,
                ConsumerState::Produced => 4,
                ConsumerState::Completed => 5,
                ConsumerState::Refused | ConsumerState::Unknown | ConsumerState::Failed => 6,
            };
            let recovered = matches!(
                consumer.state,
                ConsumerState::Unknown | ConsumerState::Refused
            ) && matches!(
                state,
                ConsumerState::Accepted | ConsumerState::Produced | ConsumerState::Completed
            );
            if recovered
                || (!matches!(
                    consumer.state,
                    ConsumerState::Completed | ConsumerState::Failed
                ) && rank(state) >= rank(&consumer.state))
            {
                consumer.state = state.clone();
            }
        }
        if self.processing_completed {
            consumer.completed_at.get_or_insert_with(|| self.at.clone());
            if !matches!(
                consumer.state,
                ConsumerState::Completed | ConsumerState::Failed
            ) {
                consumer.state = ConsumerState::Unknown;
            }
        }
        if let Some(id) = &self.response_id {
            consumer.response_id = Some(id.clone());
        }
        if let Some(value) = &self.display_name {
            consumer.display_name = Some(value.clone());
        }
        if let Some(value) = &self.provider {
            consumer.provider = Some(value.clone());
        }
        if let Some(value) = &self.conversation_id {
            consumer.conversation_id = Some(value.clone());
        }
        if let Some(value) = &self.agent_id {
            consumer.agent_id = Some(value.clone());
        }
        if let Some(delivery) = &self.delivery {
            // Bounded receipt list. Heartbeats replace their prior receipt.
            if let Some(previous) = consumer.deliveries.iter_mut().find(|previous| {
                previous.step == delivery.step
                    && previous.destination == delivery.destination
                    && previous.recipients == delivery.recipients
                    && (delivery.step == "heartbeat"
                        || previous.platform_message_id == delivery.platform_message_id)
            }) {
                if matches!(
                    delivery.step.as_str(),
                    "platform_status" | "platform_failure"
                ) {
                    let rank = |status: &str| match status {
                        "sent" => 1,
                        "delivered" => 2,
                        "read" => 3,
                        "played" => 4,
                        "failed" => 5,
                        _ => 0,
                    };
                    // Reordered or duplicate callbacks retain the strongest
                    // observation and its original gateway receive timestamp.
                    if rank(&delivery.outcome) <= rank(&previous.outcome) {
                        return;
                    }
                }
                *previous = delivery.clone();
            } else {
                consumer.deliveries.push(delivery.clone());
                if consumer.deliveries.len() > 16 {
                    consumer.deliveries.remove(0);
                }
            }
        }
    }
}
