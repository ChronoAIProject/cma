//! Native Lark/Feishu custom apps are distinct from every NyxID relay source.
mod client;
pub(crate) mod content;
pub use client::{Client, Error, Transport};
pub(crate) mod management;
pub(crate) mod message;
pub mod receipts;
pub(crate) mod webhook;
use cmaeg_core::{
    channel::{ChannelDefinition, NativeLarkCredentials, NativeLarkSource, Source},
    seal::ReplyAddress,
};
use sha2::{Digest, Sha256};
pub(crate) fn revision(source: &NativeLarkSource, credentials: &NativeLarkCredentials) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                source,
                credentials.app_secret.as_str(),
                credentials.encrypt_key.as_str(),
                credentials.verification_token.as_str()
            ))
            .expect("native revision serializes")
        )
    )
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
