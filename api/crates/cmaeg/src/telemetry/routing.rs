//! Fixed routing classifications; never expose producer data or tokens to logs.
use super::{channel_hash, names};
use crate::channels::resolver::ResolvedChannel;
use cmaeg_core::{
    activity::{RouteResolution, SourceKind},
    routing::{self, Resolution, RouteError, RouteInput},
};

/// Resolve and observe once. Interaction gates use the pure resolver, then
/// call `record` only after the store has established the final outcome.
pub fn resolve(
    channel: &ResolvedChannel,
    input: &RouteInput<'_>,
    enabled: bool,
) -> Result<Resolution, RouteError> {
    let result = routing::resolve(&channel.definition, input, enabled);
    record(channel, input, enabled, &result);
    result
}

pub fn record(
    channel: &ResolvedChannel,
    input: &RouteInput<'_>,
    enabled: bool,
    result: &Result<Resolution, RouteError>,
) {
    let shadow;
    let (trace, outcome, rule_id, targets) = match result {
        Ok(Resolution::Routed(decision)) => (
            &decision.trace,
            "routed",
            decision.routing.rule_id.as_ref().map(|id| id.as_str()),
            decision
                .routing
                .targets
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
        ),
        other => {
            // Only refusals and kill-switch legacy need the bounded replay;
            // successful routed events already carry their exact trace.
            shadow = routing::explain(&channel.definition, input, enabled);
            let (outcome, targets) = if other.is_ok() {
                ("legacy", vec!["default"])
            } else {
                ("refused", Vec::new())
            };
            (&shadow, outcome, None, targets)
        }
    };
    let kill_switch = channel.definition.routing.is_some() && !enabled;
    let stage = trace
        .selected_resolution()
        .map(resolution_name)
        .unwrap_or_else(|| {
            if input.card_consumer.is_some() && result.is_err() {
                "interaction"
            } else {
                "legacy"
            }
        });
    let resolution = if kill_switch { "kill_switch" } else { stage };
    metrics::counter!(names::ROUTING_DECISIONS, "resolution" => resolution, "outcome" => outcome)
        .increment(1);
    let channel_hash = channel_hash(channel.id.as_str());
    // Event ids are sender-controlled and may contain identifying data.
    let event_id = super::channel_hash(input.activity.event_id.as_str());
    let span = tracing::info_span!("cmaeg.route", resolution, rule_id, targets = ?targets);
    span.in_scope(|| {
        tracing::info!(%channel_hash, %event_id, resolution, outcome, targets = ?targets,
            "routing decision");
        if kill_switch && result.is_err() {
            let transport = match input.activity.source {
                SourceKind::Telegram { .. } => "telegram",
                SourceKind::NyxidRelay { .. } => "nyxid_relay",
                SourceKind::Custom => "custom",
            };
            tracing::warn!(%channel_hash, %event_id, transport, resolution = stage,
                "kill-switch routing refusal");
        }
    });
}

fn resolution_name(resolution: RouteResolution) -> &'static str {
    match resolution {
        RouteResolution::Default => "default",
        RouteResolution::Rule => "rule",
        RouteResolution::Mention => "mention",
        RouteResolution::Command => "command",
        RouteResolution::EventField => "event_field",
        RouteResolution::Interaction => "interaction",
        RouteResolution::AgentEvent => "agent_event",
    }
}
