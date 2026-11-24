//! Rollout gates graduated to default-on.
//!
//! Every gate listed here finished its reader-first rollout in each
//! environment, so an unset variable now means enabled. An explicit `false`
//! remains the emergency off-switch, and any other value keeps the existing
//! "must be true or false" error. Variables keep their names and readers.
//!
//! Some gates are refused at startup unless another gate is on. Such a gate
//! defaults on only while each of its prerequisites is itself effectively on,
//! so explicitly switching a prerequisite off never fails startup on a
//! dependent's default.
//!
//! Gates that still need a per-environment companion setting, an operator-run
//! migration or an explicit operator attestation are absent from this list and
//! stay default-off.
use super::{routing::CMAEG_ROUTED_RECOVERY_ENABLED, ConfigError};
use std::env::VarError;

pub(crate) const CMAEG_NATIVE_DISCORD_ENABLED: &str = "CMAEG_NATIVE_DISCORD_ENABLED";

/// Routed admission, durable Custom selection, quoted context and the native
/// Discord runtime are refused without durable routed recovery.
const RECOVERY: &[&str] = &[CMAEG_ROUTED_RECOVERY_ENABLED];
/// Native Discord discovery, rich content and forwards are refused without
/// the native Discord runtime and durable routed recovery.
const NATIVE_DISCORD: &[&str] = &[CMAEG_NATIVE_DISCORD_ENABLED, CMAEG_ROUTED_RECOVERY_ENABLED];
const NONE: &[&str] = &[];

/// Each graduated gate, and the gates that must be on for its unset default
/// to be on.
pub(crate) const GRADUATED: &[(&str, &[&str])] = &[
    ("CMAEG_CUSTOM_DURABLE_EXECUTION_ENABLED", RECOVERY),
    ("CMAEG_DISCORD_CARD_EDITS_ENABLED", NONE),
    ("CMAEG_DISCORD_CARD_EFFECTS_ENABLED", NONE),
    ("CMAEG_DISCORD_FORMS_ENABLED", NONE),
    ("CMAEG_DISCORD_INTERACTIONS_ENABLED", NONE),
    ("CMAEG_DISCORD_MESSAGE_EVENTS_ENABLED", NONE),
    ("CMAEG_DISCORD_MULTISELECT_ENABLED", NONE),
    ("CMAEG_DISCORD_REACTIONS_ENABLED", NONE),
    ("CMAEG_DYNAMIC_SOURCES_WRITES_ENABLED", NONE),
    ("CMAEG_GITHUB_ENABLED", NONE),
    ("CMAEG_LARK_FORMS_ENABLED", NONE),
    ("CMAEG_LARK_MESSAGE_EFFECTS_WRITES_ENABLED", NONE),
    ("CMAEG_LARK_RESOURCE_WRITES_ENABLED", NONE),
    ("CMAEG_MULTI_CONSUMER_WRITES_ENABLED", NONE),
    ("CMAEG_NATIVE_BOT_MENTIONS_WRITES_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_CONVERSATION_HISTORY_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_DISCOVERY_ENABLED", NATIVE_DISCORD),
    ("CMAEG_NATIVE_DISCORD_EDITS_ENABLED", NONE),
    (CMAEG_NATIVE_DISCORD_ENABLED, RECOVERY),
    ("CMAEG_NATIVE_DISCORD_FORWARDS_ENABLED", NATIVE_DISCORD),
    ("CMAEG_NATIVE_DISCORD_MEDIA_CARD_EDITS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_MEDIA_CARD_REPLACEMENTS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_MEDIA_CARD_SETS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_MEDIA_CONTROLS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_MEDIA_EDITS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_MEDIA_EFFECTS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_MESSAGE_EFFECTS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_RECEIVED_REACTIONS_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_RICH_CONTENT_ENABLED", NATIVE_DISCORD),
    ("CMAEG_NATIVE_DISCORD_THREAD_HISTORY_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_THREAD_LIFECYCLE_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_THREAD_METADATA_ENABLED", NONE),
    ("CMAEG_NATIVE_DISCORD_WRITES_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_CONTROLS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_CONVERSATION_HISTORY_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_FORMS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_INTAKE_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_MEDIA_CARD_REPLACEMENTS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_MEDIA_CONTROLS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_MEDIA_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_MESSAGE_EFFECTS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_MIXED_HISTORY_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_MULTI_IMAGE_CONTROLS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_OUTBOUND_MEDIA_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_RECEIVED_REACTIONS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_STICKERS_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_STICKER_REPLIES_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_THREAD_START_ENABLED", NONE),
    ("CMAEG_NATIVE_LARK_WRITES_ENABLED", NONE),
    ("CMAEG_NATIVE_MEDIA_PRESENTATIONS_ENABLED", NONE),
    ("CMAEG_NATIVE_QUOTED_CONTEXT_ENABLED", RECOVERY),
    ("CMAEG_NATIVE_SPLIT_TEXT_REFS_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_ALBUMS_ENABLED", NONE),
    (
        "CMAEG_NATIVE_TELEGRAM_GENERAL_TOPIC_LIFECYCLE_ENABLED",
        NONE,
    ),
    ("CMAEG_NATIVE_TELEGRAM_MEDIA_CONTROLS_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_MEDIA_CONTROL_EDITS_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_MEDIA_EDITS_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_MESSAGE_EVENTS_WRITES_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_RECEIVED_REACTIONS_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_STRUCTURED_REPLIES_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_TOPIC_CREATION_ENABLED", NONE),
    ("CMAEG_NATIVE_TELEGRAM_TOPIC_LIFECYCLE_ENABLED", NONE),
    ("CMAEG_NATIVE_TYPING_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_FLOW_WRITES_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_FORMS_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_GROUPS_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_GROUPS_WRITES_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_INTERACTIONS_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_MEDIA_CONTROLS_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_MEDIA_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_MESSAGE_EVENTS_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_OUTBOUND_MEDIA_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_REACTIONS_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_STRUCTURED_REPLIES_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_TEMPLATES_ENABLED", NONE),
    ("CMAEG_NATIVE_WHATSAPP_WRITES_ENABLED", NONE),
    ("CMAEG_OUTBOUND_MESSAGES_WRITES_ENABLED", NONE),
    ("CMAEG_RELAY_RAW_VERIFICATION_WRITES_ENABLED", NONE),
    ("CMAEG_ROUTED_ADMISSION_ENABLED", RECOVERY),
    (CMAEG_ROUTED_RECOVERY_ENABLED, NONE),
    ("CMAEG_TELEGRAM_MEDIA_EFFECTS_ENABLED", NONE),
    ("CMAEG_THREAD_CONTRACT_WRITES_ENABLED", NONE),
];

/// The prerequisites of a graduated gate, or `None` for a gate that is still
/// default-off.
fn prerequisites(name: &str) -> Option<&'static [&'static str]> {
    GRADUATED
        .iter()
        .find(|(graduated, _)| *graduated == name)
        .map(|(_, requires)| *requires)
}

/// Read one boolean rollout gate through `lookup`.
///
/// Exact `true` and `false` are honoured as given. Unset is off, unless the
/// gate has graduated and each of its prerequisites is effectively on.
pub(super) fn flag(
    name: &str,
    lookup: &mut impl FnMut(&str) -> Result<String, VarError>,
) -> Result<bool, ConfigError> {
    match lookup(name).as_deref() {
        Ok("true") => Ok(true),
        Ok("false") => Ok(false),
        Err(VarError::NotPresent) => {
            let Some(requires) = prerequisites(name) else {
                return Ok(false);
            };
            for prerequisite in requires {
                if !flag(prerequisite, lookup)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Err(ConfigError::InvalidManagement {
            detail: format!("{name} must be true or false"),
        }),
    }
}

#[cfg(test)]
#[path = "graduated_tests.rs"]
mod tests;
