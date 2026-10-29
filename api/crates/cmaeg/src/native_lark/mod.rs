//! Native Lark/Feishu custom apps are distinct from every NyxID relay source.
pub(crate) mod callback;
mod client;
pub(crate) mod content;
pub(crate) mod control_recovery;
pub(crate) mod controls;
pub(crate) mod effects;
pub(crate) mod history_authority;
pub(crate) mod history_projection;
pub(crate) mod history_read;
pub use client::{Client, Error, Transport};
pub(crate) mod management;
pub(crate) mod message;
pub(crate) mod outbound;
pub(crate) mod read_status;
pub mod receipts;
pub(crate) mod stickers;
pub(crate) mod thread_start;
pub(crate) mod webhook;
use cmaeg_core::{
    channel::{ChannelDefinition, NativeLarkCredentials, NativeLarkSource, Source},
    seal::ReplyAddress,
};
use sha2::{Digest, Sha256};
pub(crate) fn revision(source: &NativeLarkSource, credentials: &NativeLarkCredentials) -> String {
    // History is a separate read consent. Its admission proof binds the full source;
    // toggling it must not revoke existing messaging/media/control authority.
    let source = messaging_source(source);
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                &source,
                credentials.app_secret.as_str(),
                credentials.encrypt_key.as_str(),
                credentials.verification_token.as_str()
            ))
            .expect("native revision serializes")
        )
    )
}
fn messaging_source(source: &NativeLarkSource) -> NativeLarkSource {
    let mut source = source.clone();
    source.conversation_history = false;
    source
}
pub(crate) fn same_messaging_source(left: &Source, right: &Source) -> bool {
    match (left, right) {
        (Source::NativeLark(left), Source::NativeLark(right)) => {
            messaging_source(left) == messaging_source(right)
        }
        _ => left == right,
    }
}
pub(crate) fn credentials<'a>(
    definition: &'a ChannelDefinition,
    reply: &ReplyAddress,
) -> Option<&'a NativeLarkCredentials> {
    let ReplyAddress::NativeLark(reply) = reply else {
        return None;
    };
    let Source::NativeLark(source) = definition
        .sources
        .iter()
        .find(|s| matches!(s,Source::NativeLark(s) if s.identity == reply.identity))?
    else {
        return None;
    };
    let credentials = definition.secrets.native_lark.as_ref()?;
    (revision(source, credentials) == reply.revision).then_some(credentials)
}

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod client_tests;

#[cfg(test)]
mod content_tests;

#[cfg(test)]
mod control_tests;
#[cfg(test)]
mod control_transport_tests;
