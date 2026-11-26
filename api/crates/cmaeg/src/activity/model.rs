use cmaeg_core::activity::{ActivityConversation, Actor, SourceKind};
use serde::{Deserialize, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
use utoipa::ToSchema;

use super::ExecutionAttribution;

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
    /// A source refusal before any Consumer was selected or launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_refusal: Option<String>,
    /// Availability only: this metadata feed never copies quoted text or IDs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted_context: Option<QuotedContextSummary>,
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

impl ChannelActivity {
    pub(crate) fn metadata(
        event_id: String,
        source: SourceKind,
        actor: Actor,
        conversation: ActivityConversation,
        occurred_at: String,
        received_at: String,
    ) -> Self {
        Self {
            event_id,
            source,
            actor,
            conversation,
            occurred_at,
            received_at,
            source_refusal: None,
            quoted_context: None,
            kind: None,
            event_type: None,
            source_occurred_at: None,
            consumers: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct QuotedContextSummary {
    pub relation: cmaeg_core::activity::QuoteRelation,
    pub text_available: bool,
    pub text_truncated: bool,
    pub content_kinds: Vec<cmaeg_core::activity::QuotedContentKind>,
}
impl QuotedContextSummary {
    pub fn from_activity(activity: &cmaeg_core::activity::Activity) -> Option<Self> {
        let cmaeg_core::activity::ActivityKind::Message {
            quoted_message: Some(quote),
            ..
        } = &activity.kind
        else {
            return None;
        };
        let text = quote.content.as_ref().and_then(|c| c.text.as_ref());
        Some(Self {
            relation: quote.relation,
            text_available: text.is_some(),
            text_truncated: text.is_some_and(|t| t.truncated),
            content_kinds: quote
                .content
                .as_ref()
                .map_or_else(Vec::new, |c| c.kinds.clone()),
        })
    }
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
    /// Named terminal refusal before a provider admitted a turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_code: Option<String>,
    /// Last precise retryable provider refusal, retained even after event expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_non_admission_code: Option<String>,
    pub consumer_id: String,
    pub display_name: Option<String>,
    pub agent_id: Option<String>,
    /// Provider-verified execution reference for this response, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_reference: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schema(ignore)]
    pub(crate) execution_attributions: Vec<ExecutionAttribution>,
    pub provider: Option<String>,
    pub conversation_id: Option<String>,
    pub state: ConsumerState,
    pub dispatched_at: Option<String>,
    pub accepted_at: Option<String>,
    pub produced_at: Option<String>,
    /// Upper bound from the earliest confirmed Answer receipt bound to this
    /// response. Only projected when the exact production time is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<String>,
    pub completed_at: Option<String>,
    pub response_id: Option<String>,
    pub deliveries: Vec<ActivityDelivery>,
    /// Independent appended responses. URLs still require Agent Manager's
    /// normal response authorization; this feed grants no conversation access.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub media_responses: Vec<ActivityMediaResponse>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct ActivityMediaResponse {
    pub attachment_index: u8,
    pub kind: String,
    pub response_id: String,
    pub response_url: String,
    pub status: String,
    pub delivery: String,
    pub observed_at: String,
}
impl ActivityConsumer {
    pub(crate) fn new(id: &str) -> Self {
        Self {
            consumer_id: id.into(),
            refusal_code: None,
            last_non_admission_code: None,
            display_name: None,
            agent_id: None,
            execution_reference: None,
            execution_attributions: Vec::new(),
            provider: None,
            conversation_id: None,
            state: ConsumerState::Received,
            dispatched_at: None,
            accepted_at: None,
            produced_at: None,
            produced_by: None,
            completed_at: None,
            response_id: None,
            deliveries: Vec::new(),
            media_responses: Vec::new(),
        }
    }

    pub(crate) fn answer_production_bound(&self) -> Option<String> {
        let response_id = self.response_id.as_deref()?;
        if self.produced_at.is_some() {
            return None;
        }
        self.deliveries
            .iter()
            .filter(|delivery| delivery.answer_response_id.as_deref() == Some(response_id))
            .filter_map(ActivityDelivery::answer_confirmation)
            .min()
            .map(|(_, at)| at.to_owned())
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
    /// Exact provider response bound to this confirmed Answer at delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer_response_id: Option<String>,
    pub platform_message_id: Option<String>,
    pub recipients: Vec<ActivityRecipient>,
}

impl ActivityDelivery {
    fn answer_confirmation(&self) -> Option<(OffsetDateTime, &str)> {
        if self.step != "answer" || self.outcome != "confirmed" {
            return None;
        }
        let at = self.confirmed_at.as_deref()?;
        Some((OffsetDateTime::parse(at, &Rfc3339).ok()?, at))
    }

    fn same_delivery(&self, other: &Self) -> bool {
        self.step == other.step
            && self.destination == other.destination
            && self.recipients == other.recipients
            && (self.step == "heartbeat" || self.platform_message_id == other.platform_message_id)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, ToSchema)]
pub struct ActivityRecipient {
    pub kind: String,
    pub id: String,
    pub thread_id: Option<String>,
}

#[derive(Clone)]
pub struct Change {
    pub(crate) execution_attribution: Option<ExecutionAttribution>,
    pub refusal_code: Option<String>,
    pub last_non_admission_code: Option<String>,
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
    pub media_response: Option<ActivityMediaResponse>,
    join_retained_answer: bool,
}
impl Change {
    pub fn state(consumer: &str, state: ConsumerState, response_id: Option<String>) -> Self {
        Self {
            consumer: consumer.into(),
            execution_attribution: None,
            refusal_code: None,
            last_non_admission_code: None,
            state: Some(state),
            at: super::now(),
            response_id,
            delivery: None,
            display_name: None,
            provider: None,
            conversation_id: None,
            agent_id: None,
            processing_completed: false,
            media_response: None,
            join_retained_answer: false,
        }
    }
    pub(crate) fn execution_attribution(consumer: &str, value: ExecutionAttribution) -> Self {
        let mut change = Self::state(consumer, ConsumerState::Received, None);
        change.state = None;
        change.execution_attribution = Some(value);
        change
    }
    pub fn refused(consumer: &str, code: &str) -> Self {
        let mut change = Self::state(consumer, ConsumerState::Refused, None);
        change.refusal_code = Some(code.into());
        change
    }
    /// Processing completion does not prove a successful provider response.
    pub fn settled(consumer: &str, response_id: String) -> Self {
        let mut change = Self::state(consumer, ConsumerState::Unknown, Some(response_id));
        change.state = None;
        change.processing_completed = true;
        change
    }

    pub(crate) fn with_answer_response(mut self, response_id: Option<&str>) -> Self {
        if let (Some(delivery), Some(id)) = (&mut self.delivery, response_id) {
            if delivery.answer_confirmation().is_some() && !id.trim().is_empty() && id.len() <= 512
            {
                delivery.answer_response_id = Some(id.to_owned());
            }
        }
        self
    }

    pub(crate) fn join_retained_answer(self, response_id: &str) -> Self {
        let mut change = self.with_answer_response(Some(response_id));
        change.join_retained_answer = true;
        change
    }

    pub fn apply(&self, row: &mut ChannelActivity) {
        if !row.consumers.iter().any(|c| c.consumer_id == self.consumer) {
            if self.join_retained_answer || row.consumers.len() >= 16 {
                return;
            }
            row.consumers.push(ActivityConsumer::new(&self.consumer));
        }
        let consumer = row
            .consumers
            .iter_mut()
            .find(|c| c.consumer_id == self.consumer)
            .unwrap();
        if let Some(code) = &self.refusal_code {
            consumer.refusal_code = Some(code.clone());
            consumer.completed_at.get_or_insert_with(|| self.at.clone());
        }
        if let Some(code) = &self.last_non_admission_code {
            consumer.last_non_admission_code = Some(code.clone());
        }
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
        if let Some(response) = &self.media_response {
            if let Some(previous) = consumer
                .media_responses
                .iter_mut()
                .find(|old| old.response_id == response.response_id)
            {
                *previous = response.clone();
            } else if consumer.media_responses.len() < 64 {
                consumer.media_responses.push(response.clone());
            }
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
        if let Some(attribution) = &self.execution_attribution {
            attribution.retain(&mut consumer.execution_attributions);
        }
        if let Some(delivery) = &self.delivery {
            if self.join_retained_answer {
                // Replay can prove identity but cannot reconstruct a confirmation time.
                if let Some(id) = delivery.answer_response_id.as_ref().filter(|_| {
                    delivery
                        .platform_message_id
                        .as_ref()
                        .is_some_and(|message| !message.is_empty())
                }) {
                    let mut retained = consumer.deliveries.iter_mut().filter(|old| {
                        old.same_delivery(delivery) && old.answer_confirmation().is_some()
                    });
                    if let Some(old) = retained.next() {
                        if retained.next().is_none() && old.answer_response_id.is_none() {
                            old.answer_response_id = Some(id.clone());
                        }
                    }
                }
                return;
            }
            // Bounded receipt list. Heartbeats replace their prior receipt.
            if let Some(previous) = consumer.deliveries.iter_mut().find(|previous| {
                previous.same_delivery(delivery)
                    && previous.answer_response_id == delivery.answer_response_id
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
                // Repeated observations of one confirmed answer retain its earliest evidence.
                if delivery.answer_response_id.is_some()
                    && previous
                        .answer_confirmation()
                        .zip(delivery.answer_confirmation())
                        .is_some_and(|(earlier, later)| earlier <= later)
                {
                    return;
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
