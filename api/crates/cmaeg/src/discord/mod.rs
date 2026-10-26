//! Native Discord transport, sealed source custody and durable intake journal.
//! Source writes remain gated; application supervision and durable inbox scheduling
//! must be composed explicitly before ordinary messaging is activated.
pub(crate) mod facts;
mod inbox_reader;
mod normalize;
pub use inbox_reader::{InboxLaunch, InboxReader, VerifiedInboxLaunch};
pub(crate) mod outbound_media;
pub(crate) mod rest;
pub use rest::{DiscordRest, RestError};
mod connection;
mod discovery;
mod gateway;
mod inbox;
mod journal;
mod journal_model;
mod protocol;
mod record_store;

pub use discovery::DiscordApi;
pub use gateway::GatewayClient;
pub use journal::DurableJournal;
pub use protocol::{
    Checkpoint, Dispatch, EndReason, GatewayEnd, GatewayError, NativeDiscordJournal,
    MESSAGE_INTENTS,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod journal_fixture;
#[cfg(test)]
mod journal_tests;

#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod normalization_tests;
#[cfg(test)]
pub(crate) mod rest_fixture;
#[cfg(test)]
mod rest_tests;
