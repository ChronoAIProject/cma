//! How an event page and the Activity tab describe routing: the decision an
//! event carried, routing's refusal, and the resolver's bounded trace. Every
//! word here is fixed console text or an escaped identifier.
use cmaeg_core::{activity::RouteResolution, ids::ConsumerId};

use super::{escape, facts};
use crate::console::{
    activity::{ActivityEntry, RoutingSummary, TraceStepView, TraceView},
    summary::ChannelSummary,
};

/// The Routing section of `entry`'s page on the channel `summary` shows.
pub(crate) fn section(entry: &ActivityEntry, summary: &ChannelSummary) -> String {
    let mut html = match (&entry.refusal, &entry.routing, &entry.trace) {
        (Some(reason), _, Some(_)) => {
            format!("<p>Refused by routing: <code>{}</code>.</p>", escape(reason))
        }
        (Some(reason), _, None) => format!(
            "<p>Refused before routing: <code>{}</code>.</p>",
            escape(reason)
        ),
        (None, Some(routing), _) => decision(routing),
        (None, None, Some(_)) => "<p>Routing kept this event on the legacy path: it went to the default consumer.</p>".to_owned(),
        (None, None, None) if summary.routed => "<p class=\"empty\">No routing decision was recorded for this event: it took the legacy path to the default consumer.</p>".to_owned(),
        (None, None, None) => "<p>Legacy channel: no routing policy, so the event went to the default consumer.</p>".to_owned(),
    };
    if let Some(trace) = &entry.trace {
        html.push_str(&trace_section(trace, summary));
    }
    html
}

/// `rule <id>`, the resolution, or `legacy`: the Activity tab's routing cell.
pub(crate) fn cell(entry: &ActivityEntry) -> String {
    match (&entry.routing, &entry.refusal) {
        (Some(routing), _) => match &routing.rule_id {
            Some(rule) => format!("rule <code>{}</code>", escape(rule)),
            None => resolution_label(routing.resolution).to_owned(),
        },
        (None, Some(_)) => "—".to_owned(),
        (None, None) => "legacy".to_owned(),
    }
}

fn decision(routing: &RoutingSummary) -> String {
    let targets: Vec<String> = routing
        .targets
        .iter()
        .enumerate()
        .map(|(index, target)| {
            let primary = if index == 0 { " (primary)" } else { "" };
            format!(
                "@{} <code>{}</code>{primary}",
                escape(&target.name),
                escape(target.id.as_str())
            )
        })
        .collect();
    facts(&[
        (
            "Resolution",
            resolution_label(routing.resolution).to_owned(),
        ),
        (
            "Rule",
            routing.rule_id.as_deref().map_or_else(
                || "—".to_owned(),
                |rule| format!("<code>{}</code>", escape(rule)),
            ),
        ),
        ("Targets", targets.join(", ")),
        (
            "Source slot",
            format!("<code>{}</code>", escape(routing.source_id.as_str())),
        ),
    ])
}

fn trace_section(trace: &TraceView, summary: &ChannelSummary) -> String {
    let steps: String = trace
        .steps
        .iter()
        .map(|step| format!("<li>{}</li>", step_label(step, summary)))
        .collect();
    let truncated = if trace.truncated {
        "<p class=\"empty\">The trace stopped at its bound of 16 steps.</p>"
    } else {
        ""
    };
    let mut html = format!("<h3>Trace</h3><ol class=\"timeline\">{steps}</ol>{truncated}");
    let skipped: Vec<String> = trace
        .steps
        .iter()
        .filter_map(|step| match step {
            TraceStepView::Skipped { consumer, reason } => Some(format!(
                "<li>{}: {}</li>",
                consumer_name(consumer, summary),
                skip_reason(reason)
            )),
            _ => None,
        })
        .collect();
    if !skipped.is_empty() {
        html.push_str(&format!(
            "<h3>Skipped consumers</h3><ul>{}</ul>",
            skipped.concat()
        ));
    }
    html
}

fn step_label(step: &TraceStepView, summary: &ChannelSummary) -> String {
    match step {
        TraceStepView::Precedence { stage } => format!("decided by {}", stage_label(stage)),
        TraceStepView::Rule {
            rule_id,
            matched,
            inactive,
            failed_on,
        } => {
            let outcome = match (matched, inactive, failed_on) {
                (_, true, _) => "inactive, skipped".to_owned(),
                (true, false, _) => "matched".to_owned(),
                (false, false, Some(field)) => {
                    format!("did not match (failed on {})", escape(field))
                }
                (false, false, None) => "did not match".to_owned(),
            };
            format!("rule <code>{}</code>: {outcome}", escape(rule_id))
        }
        TraceStepView::Skipped { consumer, reason } => format!(
            "skipped {}: {}",
            consumer_name(consumer, summary),
            skip_reason(reason)
        ),
        TraceStepView::Addressing { form, tokens } => format!(
            "addressing by {} naming {tokens} consumer(s)",
            escape(&form.replace('_', " "))
        ),
        TraceStepView::Shadow {
            resolution,
            outcome,
        } => format!(
            "shadow decision under the kill switch: {} → {}",
            resolution_label(*resolution),
            match *outcome {
                "legacy" => "legacy path to the default consumer",
                "closed" => "refused",
                other => other,
            }
        ),
    }
}

pub(crate) fn resolution_label(resolution: RouteResolution) -> &'static str {
    match resolution {
        RouteResolution::Default => "default targets",
        RouteResolution::Rule => "rule",
        RouteResolution::Mention => "mention",
        RouteResolution::Command => "/to command",
        RouteResolution::EventField => "cmaegtargets event field",
        RouteResolution::Interaction => "card click",
        RouteResolution::AgentEvent => "hand-off",
    }
}

fn stage_label(stage: &str) -> String {
    match stage {
        "interaction" => "a card click".to_owned(),
        "agent_event" => "a hand-off".to_owned(),
        "addressing" => "explicit addressing".to_owned(),
        "rules" => "the rules".to_owned(),
        "default" => "the default targets".to_owned(),
        "kill_switch" => "the kill switch: multi-consumer routing is off".to_owned(),
        other => escape(other),
    }
}

fn skip_reason(reason: &str) -> String {
    match reason {
        "pending" => "pending".to_owned(),
        "ignores_actor_kind" => "does not answer non-human actors".to_owned(),
        other => escape(other),
    }
}

/// `@name <id>` when the summary knows the consumer, else its id.
fn consumer_name(id: &ConsumerId, summary: &ChannelSummary) -> String {
    let code = format!("<code>{}</code>", escape(id.as_str()));
    summary
        .consumers
        .iter()
        .find(|consumer| consumer.id == *id)
        .map_or_else(
            || code.clone(),
            |consumer| format!("@{} {code}", escape(&consumer.name)),
        )
}
