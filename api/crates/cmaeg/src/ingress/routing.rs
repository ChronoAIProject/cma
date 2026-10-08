//! Route already-admitted chat activities before their references are sealed.
mod notices;

use super::{admission::Refusal, IngressState};
use crate::{
    channels::resolver::ResolvedChannel,
    turns::observer::{ActivitySummary, RefusalSummary},
};
use cmaeg_core::{
    activity::Activity,
    routing::{Resolution, RouteError, RouteInput},
    seal::ReplyAddress,
};
use std::sync::Arc;

pub(super) fn chat(
    state: &IngressState,
    channel: &Arc<ResolvedChannel>,
    activity: &mut Activity,
    reply: &ReplyAddress,
) -> Result<(), Refusal> {
    let input = RouteInput {
        activity,
        chat_source: channel.definition.source_of(&activity.source),
        card_consumer: None,
        published: None,
        non_human_providers: &state.routing.non_human_providers,
        attributes_signed: true,
    };
    match crate::telemetry::routing::resolve(channel, &input, state.routing.multi_consumer_enabled)
    {
        Ok(Resolution::Legacy) => Ok(()),
        Ok(Resolution::Routed(decision)) => {
            activity.routing = Some(decision.routing);
            state.observer.routed(
                &channel.pointer.record_id,
                &ActivitySummary::of(activity),
                &decision.trace,
            );
            Ok(())
        }
        Err(error) => {
            let (refusal, code) = chat_refusal(&error);
            state.observer.refused(
                &channel.pointer.record_id,
                &RefusalSummary {
                    at: time::OffsetDateTime::now_utc(),
                    event_key: Some(crate::telemetry::channel_hash(activity.event_id.as_str())),
                    source: activity.source.source_id(),
                    code,
                },
            );
            notices::send(state, channel, activity, reply, &error);
            Err(refusal)
        }
    }
}

fn chat_refusal(error: &RouteError) -> (Refusal, &'static str) {
    match error {
        RouteError::TargetUnknown(_) => (Refusal::TargetUnknown, "target_unknown"),
        RouteError::TargetNotMentionable(_) => {
            (Refusal::TargetNotMentionable, "target_not_mentionable")
        }
        RouteError::TooManyTargets => (Refusal::TooManyTargets, "too_many_targets"),
        RouteError::ConsumerUnavailable | RouteError::KillSwitchClosed => {
            (Refusal::ConsumerUnavailable, "consumer_unavailable")
        }
        // Chat actors cannot reach custom-event, publish or card constraints.
        // Keep an unexpected future resolver result closed without leaking tokens.
        _ => {
            tracing::warn!(code = %error, "unexpected chat routing refusal");
            (Refusal::ConsumerUnavailable, "consumer_unavailable")
        }
    }
}
