//! Build one bounded notice; the shared delivery renderer owns transport escaping.
use crate::{channels::resolver::ResolvedChannel, ingress::IngressState};
use cmaeg_core::{
    activity::Activity, channel::ChannelDefinition, routing::RouteError, seal::ReplyAddress,
};
use std::sync::Arc;

pub(super) fn send(
    state: &IngressState,
    channel: &Arc<ResolvedChannel>,
    activity: &Activity,
    reply: &ReplyAddress,
    error: &RouteError,
) {
    let Some(policy) = &channel.definition.routing else {
        return;
    };
    if !state.routing.multi_consumer_enabled || !policy.addressing.unknown_target_notice {
        return;
    }
    let Some(text) = text(
        &channel.definition,
        &state.routing.unknown_target_text,
        error,
    ) else {
        return;
    };
    crate::delivery::notice::submit_notice(
        &*state.delivery,
        tracing::Span::current(),
        channel.clone(),
        reply.clone(),
        activity.event_id.clone(),
        text,
    );
}

fn text(definition: &ChannelDefinition, template: &str, error: &RouteError) -> Option<String> {
    let policy = definition.routing.as_ref()?;
    let mut text = match error {
        RouteError::TargetUnknown(token) | RouteError::TargetNotMentionable(token) => {
            let name = token
                .chars()
                .take(32)
                .map(|c| match c {
                    // Escaping markup alone cannot stop a transport auto-linking
                    // a naked URL, domain or username supplied as an unknown token.
                    '.' => "[.]".into(),
                    ':' => "[:]".into(),
                    '/' => "[/]".into(),
                    '@' => "[@]".into(),
                    other => other.to_string(),
                })
                .collect::<String>();
            let names = definition
                .consumers()
                .filter(|consumer| consumer.mentionable && !consumer.pending)
                .map(|consumer| consumer.name)
                .collect::<Vec<_>>()
                .join(", ");
            // Insert the untrusted token last so it cannot become a placeholder.
            template.replace("{names}", &names).replace("{name}", &name)
        }
        RouteError::TooManyTargets => format!(
            "Too many agents addressed; at most {} per message.",
            policy.max_targets
        ),
        _ => return None,
    };
    // Repeated operator placeholders must still fit one chat send after escaping.
    let mut end = text.len().min(1024);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    Some(text)
}

#[cfg(test)]
#[path = "notice_tests.rs"]
mod tests;
