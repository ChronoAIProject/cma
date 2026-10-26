//! Native Discord transport, sealed source custody and durable intake journal.
//! Source writes remain gated; outbound delivery and application-level recovery
//! integration are required before startup may activate ordinary messaging.
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
