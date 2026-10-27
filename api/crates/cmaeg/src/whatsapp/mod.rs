//! Native Meta WhatsApp Cloud API. NyxID relay integration is independent.
mod client;
pub(crate) mod management;
pub(crate) mod media;
pub(crate) mod outbound;
mod reaction;
pub mod receipts;
pub mod webhook;
pub use client::{Client, Error, Transport};

pub(crate) fn credentials<'a>(
    definition: &'a cmaeg_core::channel::ChannelDefinition,
    address: &cmaeg_core::seal::ReplyAddress,
) -> Option<&'a cmaeg_core::channel::WhatsappCredentials> {
    let cmaeg_core::seal::ReplyAddress::Whatsapp {
        app_id,
        waba_id,
        phone_number_id,
        ..
    } = address
    else {
        return None;
    };
    definition.sources.iter().any(|source| matches!(source, cmaeg_core::channel::Source::Whatsapp { app_id: app, waba_id: account, phone_number_id: phone, .. } if app == app_id && account == waba_id && phone == phone_number_id)).then_some(())?;
    definition.secrets.whatsapp.as_ref()
}

pub const GRAPH_API_VERSION: &str = "v25.0";

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
pub(crate) mod media_tests;
