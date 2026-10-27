//! Approved templates retain their typed substitutions and exact event destination.
use super::Error;
use cmaeg_core::{
    channel::whatsapp::{valid_message_id, valid_meta_id},
    message::WhatsappTemplate,
};
use serde_json::json;

/// Serialized before entering the send gate; callers cannot substitute raw JSON.
pub(crate) struct PreparedTemplate {
    pub(super) phone: String,
    pub(super) recipient: String,
    pub(super) body: Vec<u8>,
    pub(super) media: bool,
    pub(super) group: bool,
}
impl PreparedTemplate {
    pub(crate) fn new(
        phone: &str,
        recipient: &str,
        parent: &str,
        template: &WhatsappTemplate,
    ) -> Result<Self, Error> {
        Self::prepare(phone, recipient, parent, template, false)
    }
    pub(crate) fn for_group(
        reply: &cmaeg_core::channel::whatsapp::WhatsappGroupReply,
        template: &WhatsappTemplate,
    ) -> Result<Self, Error> {
        reply.validate().map_err(|_| Error::Invalid)?;
        if template.components.iter().any(|component| {
            matches!(
                component,
                cmaeg_core::message::WhatsappTemplateComponent::Button { .. }
            )
        }) {
            return Err(Error::Invalid);
        }
        Self::prepare(
            &reply.phone_number_id,
            &reply.group_id,
            &reply.message_id,
            template,
            true,
        )
    }
    fn prepare(
        phone: &str,
        recipient: &str,
        parent: &str,
        template: &WhatsappTemplate,
        group: bool,
    ) -> Result<Self, Error> {
        if !valid_meta_id(phone)
            || !(if group {
                cmaeg_core::channel::whatsapp::valid_group_id(recipient)
            } else {
                valid_meta_id(recipient)
            })
            || !valid_message_id(parent)
        {
            return Err(Error::Invalid);
        }
        template.validate().map_err(|_| Error::Invalid)?;
        validate_links(template).map_err(|_| Error::Invalid)?;
        let body = serde_json::to_vec(&json!({
            "messaging_product":"whatsapp", "recipient_type":if group {"group"}else{"individual"}, "to":recipient,
            "context":{"message_id":parent}, "type":"template", "template":template,
        }))
        .map_err(|_| Error::Invalid)?;
        // The full typed limit (20 substitutions, one bounded media link)
        // fits below this even with JSON escaping; retain a serialized bound.
        if body.len() > 256 * 1024 {
            return Err(Error::Invalid);
        }
        Ok(Self {
            phone: phone.into(),
            recipient: recipient.into(),
            body,
            media: template.has_media(),
            group,
        })
    }
}

/// Templates carry references directly to Meta; apply the same conservative
/// public-link policy before either a reply or an owner-initiated send commits.
pub(crate) fn validate_links(template: &WhatsappTemplate) -> Result<(), ()> {
    for link in template.media_links() {
        let url = crate::net::policy::target(link).ok_or(())?;
        if url.host_str().is_some_and(|host| {
            let host = host.trim_end_matches('.');
            host == "localhost"
                || [
                    ".localhost",
                    ".local",
                    ".internal",
                    ".svc",
                    ".cluster.local",
                ]
                .iter()
                .any(|suffix| host.ends_with(suffix))
        }) {
            return Err(());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "template_tests.rs"]
pub(crate) mod tests;
