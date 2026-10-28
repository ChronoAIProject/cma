//! Separate native-app card authority and durable click recovery; never a relay card.
mod authority;
mod dispatch;
pub(crate) use dispatch::current as current_dispatch;
mod click;
mod media;
mod recovery;
pub(crate) use media::{preflight_media, render_with_image, Image};
mod replacement;
mod store;
use super::{Card, Error};
pub use authority::Authority;
pub(crate) use authority::{eligible, route};
use cmaeg_core::message::{OutboundMessage, RenderingReport};
use serde_json::Value;

pub(crate) fn render(
    message: &OutboundMessage,
    id: &str,
    revision: u64,
    form: bool,
) -> Result<(Value, RenderingReport), Error> {
    if message.reply_to.is_some() || message.native_media_update.is_some() {
        return Err(Error::Denied);
    }
    // Only the caller's retained native thread can authorize this flag. Relay
    // renderers retain their existing no-thread contract.
    let mut wire = message.clone();
    wire.thread_reply = false;
    if form {
        super::lark_form::schema(&wire)?;
    }
    let (mut value, report) = if form {
        super::lark::render_form(&wire, id, revision)
    } else {
        super::lark::render(&wire, id, revision)
    }?;
    fn replace(value: &mut Value) {
        match value {
            Value::Object(values) => {
                if let Some(Value::String(locator)) = values.get_mut("cma_action") {
                    if let Some(rest) = locator.strip_prefix("clf1:") {
                        *locator = format!("cnlf1:{rest}");
                    } else if let Some(rest) = locator.strip_prefix("cl1:") {
                        *locator = format!("cnl1:{rest}");
                    }
                }
                for item in values.values_mut() {
                    replace(item);
                }
            }
            Value::Array(values) => {
                for item in values {
                    replace(item);
                }
            }
            _ => {}
        }
    }
    replace(&mut value);
    if serde_json::to_vec(&value).map_err(|_| Error::Denied)?.len() > 28 * 1024 {
        return Err(Error::Denied);
    }
    Ok((value, report))
}
pub(crate) fn markup(card: &Card) -> Result<(Value, RenderingReport), Error> {
    let authority = card.native_lark.as_ref().ok_or(Error::Denied)?;
    render_with_image(
        &card.message,
        &card.id,
        card.revision,
        authority.form,
        authority.image.as_ref(),
    )
}
pub(super) fn valid(card: &Card, keys: &cmaeg_core::seal::MasterKeyRing) -> bool {
    valid_shape(card) && (card.native_lark.is_none() || store::valid_reference(card, keys))
}
pub(super) fn valid_shape(card: &Card) -> bool {
    match &card.native_lark {
        Some(a) => a.valid(card),
        None => true,
    }
}
pub(crate) fn is_reference(message: &cmaeg_core::seal::MessageRefPayload) -> bool {
    message.interaction_record_id.is_some() && message.route_id.starts_with("native-lark-card:")
}

/// A winning callback owns the message until its exact durable admission is saved.
pub(crate) fn mutation_ready(card: &Card) -> bool {
    card.native_lark
        .as_ref()
        .is_some_and(|a| a.pending.as_ref().is_none_or(|pending| pending.admitted))
}

pub(crate) fn decode(locator: &str) -> Result<(String, usize, Option<u64>), Error> {
    if !locator.starts_with("cnl1:") && !locator.starts_with("cnlf1:") {
        return Err(Error::Denied);
    }
    let (id, index, revision) = super::card::decode(locator)?;
    let prefix = if locator.starts_with("cnlf1:") {
        "cnlf1"
    } else {
        "cnl1"
    };
    let compact = uuid::Uuid::parse_str(&id)
        .map_err(|_| Error::Denied)?
        .simple();
    if locator
        != format!(
            "{prefix}:{compact}:{}:{index}",
            revision.ok_or(Error::Denied)?
        )
    {
        return Err(Error::Denied);
    }
    Ok((id, index, revision))
}

pub(crate) fn project(
    capabilities: &mut crate::tools::Capabilities,
    channel: &crate::channels::resolver::ResolvedChannel,
    event: &cmaeg_core::seal::EventRefPayload,
    activity: &cmaeg_core::activity::Activity,
    verified: bool,
    controls: bool,
    forms: bool,
) {
    if verified && controls && eligible(channel, event, activity) {
        capabilities.buttons = "native_card".into();
        capabilities.cards = "native_card".into();
        capabilities.max_text_chars = 4000;
        capabilities.forms = Some(forms);
        capabilities.edit = true;
        capabilities.message_effects = Some(crate::tools::MessageEffectsCapabilities {
            media_edit: None,
            delete: true,
            reaction: true,
            scope: "confirmed_native_card".into(),
            max_reactions: 1,
            expires_at: None,
        });
    }
}

#[cfg(test)]
pub(crate) mod fixture;
#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod tests;
