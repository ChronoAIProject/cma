//! Source-list validation for channel definitions.

use std::collections::BTreeSet;

use super::{
    telegram,
    validation::{
        validate_admission, validate_custom_auth, validate_https_url, validate_nonempty_unique,
        validate_relay_identity,
    },
    ChannelSecrets, Source, MAX_PROMPT_TEMPLATE_BYTES, MAX_ROUTE_IDS, MAX_SOURCES,
};
use crate::{require_string, ValidationError};

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
            Source::Telegram {
                bot_id,
                bot_username,
                admission,
            } => {
                if !variants.insert("telegram") {
                    return Err(ValidationError::new(
                        &pointer,
                        "source variant is duplicated",
                    ));
                }
                telegram::validate(
                    bot_id,
                    bot_username.as_deref(),
                    admission,
                    secrets,
                    &pointer,
                )?;
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
                admission,
            } => {
                if !variants.insert("nyxid_relay") {
                    return Err(ValidationError::new(
                        &pointer,
                        "source variant is duplicated",
                    ));
                }
                if !relay_platforms.insert(*platform) {
                    return Err(ValidationError::new(&pointer, "platform is duplicated"));
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
                    &pointer,
                )?;
                validate_admission(admission, &pointer)?;
            }
            Source::Custom {
                auth,
                prompt_template,
                schema,
                attachments,
            } => {
                if has_custom {
                    return Err(ValidationError::new(
                        &pointer,
                        "source variant is duplicated",
                    ));
                }
                has_custom = true;
                variants.insert("custom");
                validate_custom_auth(auth, &pointer)?;
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
            }
        }
    }
    Ok(())
}
