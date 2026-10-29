//! Native Meta WhatsApp Cloud API. NyxID relay integration is independent.
mod client;
pub(crate) mod forms;
pub(crate) mod groups;
mod interactive;
pub(crate) mod management;
pub(crate) mod media;
pub(crate) mod message_changes;
pub(crate) mod message_context;
pub(crate) mod outbound;
mod reaction;
pub mod receipts;
pub(crate) mod shared_facts;
pub(crate) mod status;
pub(crate) mod templates;
mod typing;
pub mod webhook;
pub use client::{Client, Error, Transport};

pub(crate) fn credentials<'a>(
    definition: &'a cmaeg_core::channel::ChannelDefinition,
    address: &cmaeg_core::seal::ReplyAddress,
) -> Option<&'a cmaeg_core::channel::WhatsappCredentials> {
    use cmaeg_core::{channel::Source, seal::ReplyAddress};
    let (app_id, waba_id, phone_number_id, group) = match address {
        ReplyAddress::Whatsapp {
            app_id,
            waba_id,
            phone_number_id,
            ..
        } => (app_id, waba_id, phone_number_id, None),
        ReplyAddress::WhatsappGroup(reply) => (
            &reply.app_id,
            &reply.waba_id,
            &reply.phone_number_id,
            Some(&reply.group_id),
        ),
        _ => return None,
    };
    let source = definition.sources.iter().find(|source| {
        matches!(source, Source::Whatsapp {
            app_id: app, waba_id: account, phone_number_id: phone, group_ids, ..
        } if app == app_id && account == waba_id && phone == phone_number_id
            && group.is_none_or(|id| group_ids.contains(id)))
    })?;
    let credentials = definition.secrets.whatsapp.as_ref()?;
    if let ReplyAddress::WhatsappGroup(reply) = address {
        if group_revision(source, credentials) != reply.revision {
            return None;
        }
    }
    Some(credentials)
}

pub const GRAPH_API_VERSION: &str = "v25.0";

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
pub(crate) mod media_tests;

#[cfg(test)]
mod interactive_tests;
#[cfg(test)]
mod shared_facts_webhook_tests;
/// Bound only into the new group address; legacy private reply bytes remain stable.
pub(crate) fn group_revision(
    source: &cmaeg_core::channel::Source,
    credentials: &cmaeg_core::channel::WhatsappCredentials,
) -> String {
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                "native_whatsapp_group_revision_v1",
                legacy_source(source),
                credentials.app_secret.as_str(),
                credentials.access_token.as_str(),
                credentials.verify_token.as_str()
            ))
            .expect("group revision serializes")
        )
    )
}

/// Forms consent is independent of retained native button/list and group authority.
pub(crate) fn legacy_source(source: &cmaeg_core::channel::Source) -> cmaeg_core::channel::Source {
    // Retained group revisions serialize the typed source in declaration order.
    // A JSON Value would sort fields, or swap them on removal with preserve_order,
    // changing issued revisions even though the source remains semantically equal.
    let mut source = without_message_events(source);
    if let cmaeg_core::channel::Source::Whatsapp { forms_enabled, .. } = &mut source {
        *forms_enabled = false;
    }
    source
}

/// Keep existing independent authority hashes stable when incoming-change consent changes.
pub(crate) fn without_message_events(
    source: &cmaeg_core::channel::Source,
) -> cmaeg_core::channel::Source {
    let mut source = source.clone();
    if let cmaeg_core::channel::Source::Whatsapp { message_events, .. } = &mut source {
        *message_events = false;
    }
    source
}

#[cfg(test)]
pub(crate) mod message_changes_tests;
