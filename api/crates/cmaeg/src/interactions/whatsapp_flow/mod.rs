//! One explicit form preparation and message-attempt fence per event and consumer.
mod authority;
mod capacity;
mod click;
mod dispatch;
mod recovery;
pub(crate) use dispatch::{current as current_dispatch, shape};
mod store;
use super::Error;
pub(crate) use authority::eligible;
pub use authority::Authority;
use serde::{Deserialize, Serialize};
pub(crate) use store::slot;

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Phase {
    Prepared,
    MessageAttempted {
        key: String,
        attempt: String,
    },
    ConfirmedUnbound {
        key: String,
        receipt: crate::whatsapp::receipts::Receipt,
    },
    Bound {
        key: String,
        receipt: crate::whatsapp::receipts::Receipt,
    },
    Declined {
        key: String,
        reason: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pending {
    pub(crate) event: cmaeg_core::seal::EventRefPayload,
    pub(crate) reference: cmaeg_core::seal::EventRef,
    pub(crate) callback: crate::whatsapp::forms::callback::Callback,
    pub(crate) admitted: bool,
}

#[derive(Serialize)]
pub(crate) struct Completion {
    pub(crate) activity: cmaeg_core::activity::Activity,
    pub(crate) event: cmaeg_core::seal::EventRefPayload,
    pub(crate) reference: cmaeg_core::seal::EventRef,
    pub(crate) callback: crate::whatsapp::forms::callback::Callback,
    pub(crate) admitted: bool,
}

pub(crate) fn token(card: &super::Card) -> String {
    format!("cwf1:{}:{}", card.id.replace('-', ""), card.revision)
}

pub(crate) fn decode(token: &str) -> Result<(String, u64), Error> {
    let mut parts = token.split(':');
    if parts.next() != Some("cwf1") {
        return Err(Error::Denied);
    }
    let id =
        uuid::Uuid::parse_str(parts.next().ok_or(Error::Denied)?).map_err(|_| Error::Denied)?;
    let revision = parts
        .next()
        .ok_or(Error::Denied)?
        .parse::<u64>()
        .map_err(|_| Error::Denied)?;
    if parts.next().is_some()
        || revision != 1
        || token != format!("cwf1:{}:{revision}", id.simple())
    {
        return Err(Error::Denied);
    }
    Ok((id.to_string(), revision))
}

pub(super) fn valid(card: &super::Card, keys: &cmaeg_core::seal::MasterKeyRing) -> bool {
    card.native_whatsapp_flow.as_ref().is_none_or(|a| {
        a.valid(card)
            && a.pending
                .as_ref()
                .is_none_or(|p| click::valid_pending(card, p))
    }) && store::valid_reference(card, keys)
}
