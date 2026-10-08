//! Custom targets are authenticated attributes and never produce chat notices.
use crate::{
    channels::resolver::ResolvedChannel,
    ingress::IngressState,
    turns::observer::{ActivitySummary, RefusalSummary},
};
use cmaeg_core::{
    activity::{Activity, Attachment},
    cloudevent::CloudEvent,
    routing::{resolve, Resolution, RouteError, RouteInput},
};

#[derive(Debug)]
pub(crate) enum BuildError {
    Invalid(&'static str),
    Routing(RouteError),
}

/// Resolve only after normalization succeeds, before any Sync allocation or seal.
pub(crate) fn routed_activity(
    channel: &ResolvedChannel,
    state: &IngressState,
    ce: CloudEvent,
    attachments: Vec<Attachment>,
    attributes_signed: bool,
) -> Result<Activity, BuildError> {
    let mut activity = super::normalize(channel, ce, attachments).map_err(BuildError::Invalid)?;
    let input = RouteInput {
        activity: &activity,
        chat_source: None,
        card_consumer: None,
        published: None,
        non_human_providers: &state.routing.non_human_providers,
        attributes_signed,
    };
    match resolve(
        &channel.definition,
        &input,
        state.routing.multi_consumer_enabled,
    ) {
        Ok(Resolution::Legacy) => {}
        Ok(Resolution::Routed(decision)) => {
            activity.routing = Some(decision.routing);
            state.observer.routed(
                &channel.pointer.record_id,
                &ActivitySummary::of(&activity),
                &decision.trace,
            );
        }
        Err(error) => {
            state.observer.refused(
                &channel.pointer.record_id,
                &RefusalSummary {
                    at: time::OffsetDateTime::now_utc(),
                    event_key: Some(crate::telemetry::channel_hash(activity.event_id.as_str())),
                    source: activity.source.source_id(),
                    code: code(&error),
                },
            );
            return Err(BuildError::Routing(error));
        }
    }
    Ok(activity)
}

fn code(error: &RouteError) -> &'static str {
    match error {
        RouteError::TargetUnknown(_) => "target_unknown",
        RouteError::TooManyTargets => "too_many_targets",
        RouteError::InvalidTargets => "invalid_targets",
        RouteError::TargetsNotPermitted => "targets_not_permitted",
        RouteError::TargetsUnsigned => "targets_unsigned",
        RouteError::TargetIgnoresActorKind(_) => "target_ignores_actor_kind",
        RouteError::NoTargetAcceptsActorKind => "no_target_accepts_actor_kind",
        RouteError::ConsumerUnavailable | RouteError::KillSwitchClosed => "consumer_unavailable",
        RouteError::TargetNotMentionable(_) | RouteError::InteractionDenied => {
            "consumer_unavailable"
        }
    }
}
