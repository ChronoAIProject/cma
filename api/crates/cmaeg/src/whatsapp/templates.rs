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
}
impl PreparedTemplate {
    pub(crate) fn new(
        phone: &str,
        recipient: &str,
        parent: &str,
        template: &WhatsappTemplate,
    ) -> Result<Self, Error> {
        if !valid_meta_id(phone) || !valid_meta_id(recipient) || !valid_message_id(parent) {
            return Err(Error::Invalid);
        }
        template.validate().map_err(|_| Error::Invalid)?;
        validate_links(template).map_err(|_| Error::Invalid)?;
        let body = serde_json::to_vec(&json!({
            "messaging_product":"whatsapp", "recipient_type":"individual", "to":recipient,
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
