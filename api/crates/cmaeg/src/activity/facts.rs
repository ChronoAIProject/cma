use super::{ActivityDelivery, ActivityRecipient, Change, ConsumerState};
use crate::{
    delivery::{destination::DefaultDestination, intents::DeliveryOutcome},
    turns::observer::{DeliveryStep, Destination},
};
use cmaeg_core::seal::ReplyAddress;

pub fn delivery(
    consumer: &str,
    step: DeliveryStep,
    outcome: &DeliveryOutcome,
    destination: Destination,
    reply: &ReplyAddress,
    default: Option<&DefaultDestination>,
) -> Change {
    let at = super::now();
    let recipients = match default {
        Some(DefaultDestination::Telegram {
            chat_id, thread_id, ..
        }) => vec![recipient("telegram_chat", chat_id, thread_id.clone())],
        Some(DefaultDestination::Relay { route_id, .. }) => {
            vec![recipient("relay_route", route_id, None)]
        }
        Some(DefaultDestination::Webhooks { sinks }) => {
            sinks.iter().filter_map(|s| webhook(&s.url)).collect()
        }
        None if matches!(destination, Destination::Origin) => match reply {
            ReplyAddress::Telegram {
                chat_id, thread_id, ..
            } => vec![recipient("telegram_chat", chat_id, thread_id.clone())],
            ReplyAddress::Discord { channel_id, .. } => {
                vec![recipient("discord_channel", channel_id, None)]
            }
            ReplyAddress::NyxidRelay { route_id, .. } => {
                vec![recipient("relay_route", route_id, None)]
            }
            ReplyAddress::ResultWebhook { url, .. } => webhook(url).into_iter().collect(),
            ReplyAddress::Sync { .. } => vec![recipient("http_stream", "origin_request", None)],
            ReplyAddress::None => Vec::new(),
        },
        None => Vec::new(),
    };
    let buffered = matches!(reply, ReplyAddress::Sync { .. })
        && default.is_none()
        && matches!(
            outcome,
            DeliveryOutcome::Confirmed {
                platform_message_id: None
            }
        );
    let confirmed = matches!(outcome, DeliveryOutcome::Confirmed { .. }) && !buffered;
    let platform_message_id = match outcome {
        DeliveryOutcome::Confirmed {
            platform_message_id,
        } => platform_message_id.clone(),
        _ => None,
    };
    let step = match step {
        DeliveryStep::Ack => "ack",
        DeliveryStep::Heartbeat => "heartbeat",
        DeliveryStep::Answer => "answer",
        DeliveryStep::ToolReply => "tool_reply",
        DeliveryStep::Failure => "failure",
        DeliveryStep::DefaultReply => "default_reply",
    };
    let mut change = Change::state(consumer, ConsumerState::Received, None);
    change.state = None;
    change.delivery = Some(ActivityDelivery {
        step: step.into(),
        destination: match destination {
            Destination::Origin => "origin",
            Destination::Default => "default",
        }
        .into(),
        outcome: if buffered { "buffered" } else { outcome.kind() }.into(),
        observed_at: at.clone(),
        confirmed_at: confirmed.then_some(at),
        platform_message_id,
        recipients,
    });
    change
}
fn recipient(kind: &str, id: &str, thread_id: Option<String>) -> ActivityRecipient {
    ActivityRecipient {
        kind: kind.into(),
        id: id.into(),
        thread_id,
    }
}
fn webhook(raw: &str) -> Option<ActivityRecipient> {
    let url = reqwest::Url::parse(raw).ok()?;
    Some(recipient(
        "webhook_origin",
        &url.origin().ascii_serialization(),
        None,
    ))
}
