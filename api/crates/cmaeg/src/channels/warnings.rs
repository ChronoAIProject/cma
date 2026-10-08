//! Advisory routing checks over the retained definition and operator capability sets.
use crate::MediaToolProviders;
use cmaeg_core::{
    channel::{ActivityKindName, ChannelDefinition, RoutingRule, Sink, Source},
    ids::ConsumerId,
};
use serde::Serialize;
use std::collections::BTreeSet;
use utoipa::ToSchema;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct Warning {
    pub pointer: String,
    pub reason: WarningReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WarningReason {
    TargetIgnoresCustomEvents,
    PublishTargetIgnoresAgentEvents,
    PublishToolsUnavailable,
    MentionableBypassesRules,
    FanoutRequiresStreamAll,
    FanoutOutputsUndeliverable,
}

pub fn for_channel(
    definition: &ChannelDefinition,
    non_human: &BTreeSet<String>,
    tools: &MediaToolProviders,
) -> Vec<Warning> {
    let Some(routing) = &definition.routing else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    let custom = definition
        .sources
        .iter()
        .any(|source| matches!(source, Source::Custom { .. }));
    let fallback = routing.default_reply.is_some()
        || definition
            .reply
            .sinks
            .iter()
            .any(|sink| matches!(sink, Sink::ResultWebhook { .. }));
    if custom {
        targets(
            definition,
            non_human,
            &routing.default_targets,
            "/routing/default_targets",
            fallback,
            &mut warnings,
        );
        for (index, rule) in routing
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| custom_rule(rule))
        {
            targets(
                definition,
                non_human,
                &rule.targets,
                &format!("/routing/rules/{index}/targets"),
                fallback,
                &mut warnings,
            );
        }
    }
    let addresses = routing.addressing.mentions || routing.addressing.command;
    for (index, consumer) in definition.consumers().enumerate() {
        let prefix = if index == 0 {
            "/routing/default_consumer".into()
        } else {
            format!("/routing/consumers/{}", index - 1)
        };
        if consumer.publish.enabled && !tools.contains(&consumer.provider.slug) {
            push(
                &mut warnings,
                format!("{prefix}/publish/enabled"),
                WarningReason::PublishToolsUnavailable,
            );
        }
        if consumer.publish.targets.is_empty() {
            if consumer.publish.enabled
                && definition.consumers().any(|target| {
                    target.id != consumer.id
                        && !target.pending
                        && !target.non_human_effective(non_human)
                })
            {
                push(
                    &mut warnings,
                    format!("{prefix}/publish/targets"),
                    WarningReason::PublishTargetIgnoresAgentEvents,
                );
            }
        } else {
            for (position, target) in consumer.publish.targets.iter().enumerate() {
                if ignores(definition, non_human, target) {
                    push(
                        &mut warnings,
                        format!("{prefix}/publish/targets/{position}"),
                        WarningReason::PublishTargetIgnoresAgentEvents,
                    );
                }
            }
        }
        if addresses
            && consumer.mentionable
            && !consumer.pending
            && routing.rules.iter().any(|rule| {
                chat_rule(definition, rule, routing.addressing.command)
                    && !rule.targets.contains(consumer.id)
            })
        {
            push(
                &mut warnings,
                format!("{prefix}/mentionable"),
                WarningReason::MentionableBypassesRules,
            );
        }
    }
    warnings
}

fn targets(
    definition: &ChannelDefinition,
    non_human: &BTreeSet<String>,
    targets: &[ConsumerId],
    prefix: &str,
    fallback: bool,
    warnings: &mut Vec<Warning>,
) {
    for (index, target) in targets.iter().enumerate() {
        if ignores(definition, non_human, target) {
            push(
                warnings,
                format!("{prefix}/{index}"),
                WarningReason::TargetIgnoresCustomEvents,
            );
        }
    }
    if targets.len() > 1 && !fallback {
        push(warnings, prefix, WarningReason::FanoutRequiresStreamAll);
        push(warnings, prefix, WarningReason::FanoutOutputsUndeliverable);
    }
}

fn ignores(
    definition: &ChannelDefinition,
    non_human: &BTreeSet<String>,
    target: &ConsumerId,
) -> bool {
    definition
        .consumer(Some(target))
        .is_some_and(|consumer| !consumer.non_human_effective(non_human))
}

fn custom_rule(rule: &RoutingRule) -> bool {
    let when = &rule.when;
    (when.sources.is_empty()
        || when
            .sources
            .iter()
            .any(|source| source.as_str() == "custom"))
        && (when.kinds.is_empty() || when.kinds.contains(&ActivityKindName::Event))
}

fn chat_rule(definition: &ChannelDefinition, rule: &RoutingRule, commands: bool) -> bool {
    let when = &rule.when;
    when.event_types.is_empty()
        && when.event_sources.is_empty()
        && (when.kinds.is_empty()
            || when.kinds.contains(&ActivityKindName::Message)
            || (commands && when.kinds.contains(&ActivityKindName::Command)))
        && definition.sources.iter().any(|source| {
            !matches!(source, Source::Custom { .. })
                && (when.sources.is_empty() || when.sources.contains(&source.source_id()))
        })
}

fn push(warnings: &mut Vec<Warning>, pointer: impl Into<String>, reason: WarningReason) {
    warnings.push(Warning {
        pointer: pointer.into(),
        reason,
    });
}

#[cfg(test)]
#[path = "warnings_tests.rs"]
mod tests;
