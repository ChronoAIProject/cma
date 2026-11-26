//! Build and validate the source list and secrets of a channel definition.

use std::collections::BTreeSet;

use cmaeg_core::{
    channel::{ChannelDefinition, ChannelSecrets, Source},
    ValidationError,
};

use super::definition::hash_secret;
use super::requests::{CredentialsRequest, SourceRequest};

/// Refuse a new or changed GitHub source while GitHub writes are disabled.
/// Management writes and offline import both apply this policy.
pub(crate) fn check_github_sources(
    current: Option<&ChannelDefinition>,
    next: &ChannelDefinition,
    enabled: bool,
) -> Result<(), ValidationError> {
    if !enabled
        && next.sources.iter().any(|source| {
            matches!(source, Source::Github { .. })
                && !current.is_some_and(|old| old.sources.contains(source))
        })
    {
        return Err(ValidationError::new("/sources", "github_writes_disabled"));
    }
    Ok(())
}

pub(super) fn build_sources(
    sources: &[SourceRequest],
    current: Option<&ChannelDefinition>,
) -> Result<Vec<Source>, ValidationError> {
    let mut result = Vec::with_capacity(sources.len());
    for (index, source) in sources.iter().enumerate() {
        let pointer = format!("/sources/{index}");
        if let SourceRequest::NyxidRelay {
            issuer,
            platform,
            trust_normalized,
            verify_raw_event,
            ..
        } = source
        {
            if *verify_raw_event
                && (*trust_normalized
                    || !matches!(
                        platform,
                        cmaeg_core::channel::Platform::Discord
                            | cmaeg_core::channel::Platform::Whatsapp
                    ))
            {
                return Err(ValidationError::new(
                    format!("{pointer}/verify_raw_event"),
                    "requires Discord or WhatsApp without trust_normalized",
                ));
            }
            // Refuse up front: a stored source like this would admit no callbacks,
            // so a bot moved onto it would go silent. Ingress still refuses one as
            // defense in depth for definitions stored before this check existed.
            if !platform.verifies_raw_event() && !trust_normalized && !verify_raw_event {
                return Err(ValidationError::new(
                    format!("{pointer}/trust_normalized"),
                    "the platform has no raw-event verifier; set trust_normalized to true to admit its normalized callbacks",
                ));
            }
            let parsed = reqwest::Url::parse(issuer).map_err(|_| {
                ValidationError::new(format!("{pointer}/issuer"), "must be an HTTPS origin")
            })?;
            if parsed.scheme() != "https"
                || parsed.host_str().is_none()
                || parsed.username() != ""
                || parsed.password().is_some()
                || parsed.query().is_some()
                || parsed.fragment().is_some()
                || parsed.path() != "/"
            {
                return Err(ValidationError::new(
                    format!("{pointer}/issuer"),
                    "must be an HTTPS origin",
                ));
            }
        }
        result.push(match source {
            SourceRequest::NativeLark(source) => Source::NativeLark(source.clone()),
            SourceRequest::Whatsapp {
                message_events,
                forms_enabled,
                group_ids,
                app_id,
                waba_id,
                phone_number_id,
                admission,
            } => Source::Whatsapp {
                message_events: *message_events,
                forms_enabled: *forms_enabled,
                group_ids: group_ids.clone(),
                app_id: app_id.clone(),
                waba_id: waba_id.clone(),
                phone_number_id: phone_number_id.clone(),
                admission: admission.clone(),
            },
            SourceRequest::Telegram {
                topic_lifecycle,
                topic_creation,
                message_events,
                channel_posts,
                member_join_chat_ids,
                bot_id,
                bot_username,
                admission,
            } => {
                let saved_member_joins = current.and_then(|definition| {
                    definition.sources.iter().find_map(|source| match source {
                        Source::Telegram {
                            bot_id: saved_bot,
                            message_events: true,
                            member_join_chat_ids,
                            ..
                        } if saved_bot == bot_id => Some(member_join_chat_ids),
                        _ => None,
                    })
                });
                Source::Telegram {
                    topic_lifecycle: *topic_lifecycle,
                    topic_creation: *topic_creation,
                    message_events: *message_events,
                    channel_posts: *channel_posts,
                    member_join_chat_ids: if !message_events && saved_member_joins.is_some() {
                        Vec::new()
                    } else {
                        member_join_chat_ids
                            .as_ref()
                            .or(saved_member_joins)
                            .cloned()
                            .unwrap_or_default()
                    },
                    bot_id: bot_id.clone(),
                    bot_username: bot_username.clone(),
                    admission: admission.clone(),
                }
            }
            SourceRequest::Discord {
                bot_id,
                intents,
                thread_history,
                thread_lifecycle,
                conversation_history,
                admission,
            } => Source::Discord {
                bot_id: bot_id.clone(),
                intents: *intents,
                thread_history: *thread_history,
                thread_lifecycle: *thread_lifecycle,
                conversation_history: *conversation_history,
                admission: admission.clone(),
            },
            SourceRequest::Github {
                app_ref,
                installation_ids,
                repository_ids,
                deny_repository_ids,
                events,
                actions,
                prompt_template,
            } => Source::Github {
                app_ref: app_ref.clone(),
                installation_ids: installation_ids.clone(),
                repository_ids: repository_ids.clone(),
                deny_repository_ids: deny_repository_ids.clone(),
                events: events.clone(),
                actions: actions.clone(),
                prompt_template: prompt_template.clone(),
            },
            SourceRequest::NyxidRelay {
                issuer,
                key_id,
                route_ids,
                platform,
                bot_username,
                bot_id,
                trust_normalized,
                verify_raw_event,
                lark_message_effects,
                lark_resources,
                member_join_chat_ids,
                admission,
            } => Source::NyxidRelay {
                issuer: issuer.clone(),
                jwks_url: format!("{}/.well-known/jwks.json", issuer.trim_end_matches('/')),
                key_id: key_id.clone(),
                route_ids: route_ids.clone(),
                platform: *platform,
                bot_username: bot_username.clone(),
                bot_id: bot_id.clone(),
                trust_normalized: *trust_normalized,
                verify_raw_event: *verify_raw_event,
                lark_message_effects: lark_message_effects.clone(),
                lark_resources: lark_resources.clone(),
                member_join_chat_ids: member_join_chat_ids.clone().unwrap_or_else(|| {
                    current
                        .and_then(|definition| {
                            definition.sources.iter().find_map(|source| match source {
                                Source::NyxidRelay {
                                    issuer: saved_issuer,
                                    key_id: saved_key,
                                    route_ids: saved_routes,
                                    bot_id: saved_bot,
                                    platform: saved_platform,
                                    member_join_chat_ids,
                                    ..
                                } if saved_issuer == issuer
                                    && saved_key == key_id
                                    && saved_routes == route_ids
                                    && saved_bot == bot_id
                                    && saved_platform == platform =>
                                {
                                    Some(member_join_chat_ids.clone())
                                }
                                _ => None,
                            })
                        })
                        .unwrap_or_default()
                }),
                admission: admission.clone(),
            },
            SourceRequest::Custom {
                auth,
                prompt_template,
                schema,
                attachments,
            } => Source::Custom {
                auth: auth.clone(),
                prompt_template: prompt_template.clone(),
                schema: schema.clone(),
                attachments: attachments.clone(),
            },
        });
        if result.len() > 4 {
            return Err(ValidationError::new(pointer, "too many sources"));
        }
    }
    Ok(result)
}

pub(super) fn build_secrets(credentials: &CredentialsRequest) -> ChannelSecrets {
    ChannelSecrets {
        profile_revisions: None,
        native_lark: credentials.native_lark.clone(),
        whatsapp: credentials.whatsapp.clone(),
        agent_key_hash: hash_secret(credentials.agent_key.as_str()),
        channel_key: credentials.channel_key.clone(),
        hmac_secrets: credentials
            .hmac_secrets
            .iter()
            .map(|secret| (secret.key_id.clone(), secret.secret.clone()))
            .collect(),
        provider_credential: Some(credentials.agent_key.clone()),
        telegram_bot_token: credentials.telegram_bot_token.clone(),
        discord_bot_token: credentials.discord_bot_token.clone(),
        telegram_webhook_secret: credentials.telegram_webhook_secret.clone(),
        consumer_keys: Vec::new(),
    }
}

pub(super) fn validate_credentials(
    credentials: &CredentialsRequest,
) -> Result<(), ValidationError> {
    for (pointer, value) in [
        ("/credentials/agent_key", Some(&credentials.agent_key)),
        ("/credentials/channel_key", credentials.channel_key.as_ref()),
    ] {
        if let Some(value) = value {
            if value.as_str().is_empty()
                || value.as_str().len() > 16_384
                || !value
                    .as_str()
                    .bytes()
                    .all(|byte| (33..=126).contains(&byte))
            {
                return Err(ValidationError::new(
                    pointer,
                    "must contain 1..=16384 printable bearer bytes",
                ));
            }
        }
    }
    if credentials.discord_bot_token.as_ref().is_some_and(|token| {
        token.as_str().is_empty()
            || token.as_str().len() > 2048
            || !token.as_str().bytes().all(|b| (33..=126).contains(&b))
    }) {
        return Err(ValidationError::new(
            "/credentials/discord_bot_token",
            "requires 1..=2048 printable bearer bytes",
        ));
    }
    let mut keys = BTreeSet::new();
    for (index, secret) in credentials.hmac_secrets.iter().enumerate() {
        if secret.key_id.is_empty() || secret.key_id.len() > 512 {
            return Err(ValidationError::new(
                format!("/credentials/hmac_secrets/{index}/key_id"),
                "must contain 1..=512 bytes",
            ));
        }
        if secret.secret.as_str().is_empty() || secret.secret.as_str().len() > 4_096 {
            return Err(ValidationError::new(
                format!("/credentials/hmac_secrets/{index}/secret"),
                "must contain 1..=4096 bytes",
            ));
        }
        use base64::Engine as _;
        if base64::engine::general_purpose::STANDARD
            .decode(secret.secret.as_str())
            .is_err()
        {
            return Err(ValidationError::new(
                format!("/credentials/hmac_secrets/{index}/secret"),
                "must be base64",
            ));
        }
        if !keys.insert(&secret.key_id) {
            return Err(ValidationError::new(
                "/credentials/hmac_secrets",
                "key ids must be unique",
            ));
        }
    }
    Ok(())
}
