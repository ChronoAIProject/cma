//! Source-list validation for channel definitions.

use std::collections::BTreeSet;

use super::{
    telegram,
    validation::{
        validate_admission, validate_custom_auth, validate_https_url, validate_nonempty_unique,
        validate_relay_identity,
    },
    ChannelSecrets, Platform, Source, MAX_EXTENDED_SOURCES, MAX_PROMPT_TEMPLATE_BYTES,
    MAX_ROUTE_IDS, MAX_SOURCES,
};
use crate::{require_string, ValidationError};

const DUPLICATED: &str = "source variant is duplicated";

/// Today's source rules, kept verbatim as the legacy predicate: 1..=4 sources,
/// at most one native Telegram, one NyxID relay of any platform and one custom source.
pub(super) fn validate_legacy_sources(
    sources: &[Source],
    secrets: &ChannelSecrets,
) -> Result<(), ValidationError> {
    if sources.is_empty() || sources.len() > MAX_SOURCES {
        return Err(ValidationError::new(
            "/sources",
            "must contain one to four sources",
        ));
    }
    let mut variants = BTreeSet::new();
    let mut relay_platforms = BTreeSet::new();
    let mut has_custom = false;
    for (index, source) in sources.iter().enumerate() {
        let pointer = format!("/sources/{index}");
        match source {
            Source::NativeLark(_) => {
                return Err(ValidationError::new(
                    &pointer,
                    "native_lark_writes_disabled",
                ))
            }
            Source::Whatsapp { .. } => {
                return Err(ValidationError::new(
                    &pointer,
                    "native_whatsapp_writes_disabled",
                ))
            }
            Source::Discord { .. } => {
                return Err(ValidationError::new(
                    &pointer,
                    "native_discord_writes_disabled",
                ));
            }
            Source::Telegram { .. } => {
                if !variants.insert("telegram") {
                    return Err(ValidationError::new(&pointer, DUPLICATED));
                }
            }
            Source::NyxidRelay { platform, .. } => {
                if !variants.insert("nyxid_relay") {
                    return Err(ValidationError::new(&pointer, DUPLICATED));
                }
                if !relay_platforms.insert(*platform) {
                    return Err(ValidationError::new(&pointer, "platform is duplicated"));
                }
            }
            Source::Custom { .. } => {
                if has_custom {
                    return Err(ValidationError::new(&pointer, DUPLICATED));
                }
                has_custom = true;
                variants.insert("custom");
            }
        }
        validate_source(source, secrets, &pointer)?;
    }
    Ok(())
}

/// The reader's source rules, applied when the legacy rules refuse a list:
/// 0..=8 sources with at most one native Telegram, one custom and one relay
/// per platform. Relays answer through one channel key, and NyxID binds that
/// key to its signing identity, so every relay shares `issuer`, `jwks_url`
/// and `key_id`; each route belongs to one relay, so a callback's route
/// selects its source unambiguously.
pub(super) fn validate_extended_sources(
    sources: &[Source],
    secrets: &ChannelSecrets,
) -> Result<(), ValidationError> {
    if sources.len() > MAX_EXTENDED_SOURCES {
        return Err(ValidationError::new(
            "/sources",
            "must contain at most eight sources",
        ));
    }
    let (mut telegram, mut discord, mut whatsapp, mut custom) = (false, false, false, false);
    let mut native_lark = false;
    let mut platforms = BTreeSet::<Platform>::new();
    let mut routes = BTreeSet::<&str>::new();
    let mut identity: Option<(&str, &str, &str)> = None;
    for (index, source) in sources.iter().enumerate() {
        let pointer = format!("/sources/{index}");
        let duplicated = match source {
            Source::NativeLark(_) => std::mem::replace(&mut native_lark, true),
            Source::Telegram { .. } => std::mem::replace(&mut telegram, true),
            Source::Discord { .. } => std::mem::replace(&mut discord, true),
            Source::Whatsapp { .. } => std::mem::replace(&mut whatsapp, true),
            Source::Custom { .. } => std::mem::replace(&mut custom, true),
            Source::NyxidRelay { .. } => false,
        };
        if duplicated {
            return Err(ValidationError::new(&pointer, DUPLICATED));
        }
        if let Source::NyxidRelay { platform, .. } = source {
            if !platforms.insert(*platform) {
                return Err(ValidationError::new(&pointer, "platform is duplicated"));
            }
        }
        validate_source(source, secrets, &pointer)?;
        let Source::NyxidRelay {
            issuer,
            jwks_url,
            key_id,
            route_ids,
            ..
        } = source
        else {
            continue;
        };
        let signer = (issuer.as_str(), jwks_url.as_str(), key_id.as_str());
        if *identity.get_or_insert(signer) != signer {
            return Err(ValidationError::new(
                format!("{pointer}/key_id"),
                "relay sources must share issuer, jwks_url and key_id",
            ));
        }
        for route in route_ids {
            if !routes.insert(route) {
                return Err(ValidationError::new(
                    format!("{pointer}/route_ids"),
                    "route id is already used by another relay source",
                ));
            }
        }
    }
    Ok(())
}

/// One source's own rules, shared by the legacy and extended predicates.
fn validate_source(
    source: &Source,
    secrets: &ChannelSecrets,
    pointer: &str,
) -> Result<(), ValidationError> {
    if source.native_bot_mentions() {
        let identity = match source {
            Source::Telegram {
                bot_id,
                bot_username,
                ..
            } => !bot_id.is_empty() && bot_username.is_some(),
            Source::NyxidRelay {
                platform,
                bot_id,
                bot_username,
                trust_normalized,
                ..
            } => {
                !trust_normalized
                    && match platform {
                        Platform::Telegram => {
                            bot_id.as_deref().is_some_and(|id| {
                                id.parse::<i64>()
                                    .is_ok_and(|n| n > 0 && n.to_string() == id)
                            }) && bot_username.is_some()
                        }
                        Platform::Lark | Platform::Feishu => bot_id
                            .as_deref()
                            .is_some_and(|id| id.starts_with("ou_") && id.len() > 3),
                        _ => false,
                    }
            }
            _ => false,
        };
        if !identity {
            return Err(ValidationError::new(
                format!("{pointer}/admission/bot_messages"),
                "native bot mentions require a raw verifier and pinned receiving identity",
            ));
        }
    }
    match source {
        Source::NativeLark(source) => source.validate(secrets.native_lark.as_ref()),
        Source::Whatsapp {
            group_ids,
            app_id,
            waba_id,
            phone_number_id,
            admission,
            ..
        } => super::whatsapp::validate(
            group_ids,
            app_id,
            waba_id,
            phone_number_id,
            admission,
            secrets.whatsapp.as_ref(),
            pointer,
        ),
        Source::Discord {
            bot_id,
            intents,
            admission,
            ..
        } => super::discord::validate(bot_id, *intents, admission, secrets, pointer),
        Source::Telegram {
            topic_lifecycle: _,
            topic_creation: _,
            bot_id,
            bot_username,
            admission,
            message_events,
            channel_posts,
        } => {
            if *channel_posts && !message_events {
                return Err(ValidationError::new(
                    format!("{pointer}/channel_posts"),
                    "native channel posts require retained message_events routing",
                ));
            }
            telegram::validate(bot_id, bot_username.as_deref(), admission, secrets, pointer)
        }
        Source::NyxidRelay {
            issuer,
            jwks_url,
            key_id,
            route_ids,
            platform,
            bot_username,
            bot_id,
            trust_normalized,
            verify_raw_event,
            lark_message_effects,
            lark_resources,
            admission,
        } => {
            if let Some(binding) = lark_message_effects {
                let field = format!("{pointer}/lark_message_effects");
                binding.validate(&field)?;
                if super::lark_resources::lark_resource_service(*platform).is_none()
                    || *trust_normalized
                    || !bot_id.as_deref().is_some_and(|id| {
                        id.starts_with("ou_") && super::lark_resources::safe_id(id, 128)
                    })
                    || secrets.channel_key.is_none()
                {
                    return Err(ValidationError::new(field, "requires a verified Lark/Feishu source, pinned bot open_id and channel key"));
                }
            }
            if let Some(binding) = lark_resources {
                let field = format!("{pointer}/lark_resources");
                binding.validate(&field)?;
                if super::lark_resources::lark_resource_service(*platform).is_none()
                    || *trust_normalized
                    || !bot_id.as_deref().is_some_and(|id| {
                        id.starts_with("ou_") && super::lark_resources::safe_id(id, 128)
                    })
                    || secrets.channel_key.is_none()
                {
                    return Err(ValidationError::new(field, "requires a verified Lark/Feishu source, independently pinned bot open_id and channel key"));
                }
            }
            require_string(issuer, &format!("{pointer}/issuer"), 1, 512)?;
            require_string(key_id, &format!("{pointer}/key_id"), 1, 512)?;
            validate_https_url(jwks_url, &format!("{pointer}/jwks_url"))?;
            // An unbound source admits no callbacks until its owner attaches routes.
            if !route_ids.is_empty() {
                validate_nonempty_unique(
                    route_ids,
                    &format!("{pointer}/route_ids"),
                    MAX_ROUTE_IDS,
                )?;
            }
            validate_relay_identity(
                *platform,
                bot_username.as_deref(),
                bot_id.as_deref(),
                *trust_normalized,
                *verify_raw_event,
                pointer,
            )?;
            validate_admission(admission, pointer)
        }
        Source::Custom {
            auth,
            prompt_template,
            schema,
            attachments,
        } => {
            validate_custom_auth(auth, pointer)?;
            if let Some(prompt) = prompt_template {
                require_string(
                    prompt,
                    &format!("{pointer}/prompt_template"),
                    0,
                    MAX_PROMPT_TEMPLATE_BYTES,
                )?;
            }
            if schema.is_some() {
                return Err(ValidationError::new(
                    format!("{pointer}/schema"),
                    "must be null in v1",
                ));
            }
            if let Some(attachments) = attachments {
                attachments.validate(&format!("{pointer}/attachments"))?;
            }
            Ok(())
        }
    }
}
