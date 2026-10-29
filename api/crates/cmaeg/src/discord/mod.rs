//! Native Discord transport, sealed source custody and durable intake journal.
//! Source writes remain gated; application supervision and durable inbox scheduling
//! must be composed explicitly before ordinary messaging is activated.
pub mod activation;
pub(crate) mod component_thread;
#[cfg(test)]
mod component_thread_tests;
mod components;
#[cfg(test)]
mod components_execution_tests;
#[cfg(test)]
pub(crate) mod components_fixture;
#[cfg(test)]
mod components_restore_tests;
#[cfg(test)]
mod components_tests;
pub(crate) mod facts;
pub(crate) mod history_authority;
pub mod inbox_dispatch;
mod inbox_reader;
mod inbox_scan;
pub(crate) mod lifecycle;
mod modal;
#[cfg(test)]
pub(crate) mod modal_fixture;
#[cfg(test)]
mod modal_race_tests;
#[cfg(test)]
mod modal_recovery_tests;
#[cfg(test)]
mod modal_tests;
#[cfg(test)]
mod multiselect_tests;
pub(crate) mod mutation;
mod normalize;
mod runtime_source;
pub mod selection;
mod source_discovery;
mod source_worker;
pub(crate) mod thread_authority;
mod thread_context;
pub(crate) mod voice;
pub use inbox_reader::{InboxLaunch, InboxReader, VerifiedInboxLaunch};
pub(crate) mod outbound_media;
pub(crate) mod rest;
pub use rest::{DiscordRest, RestError};
mod connection;
mod discovery;
mod gateway;
mod inbox;
mod inbox_activity;
mod journal;
mod journal_model;
mod message_event;
mod message_event_inbox;
mod protocol;
mod reaction_inbox;
mod reaction_pending;
mod reactions;
pub(crate) mod receipts;
mod record_store;

pub use discovery::DiscordApi;
pub use gateway::GatewayClient;
pub use journal::DurableJournal;
pub use protocol::{
    Checkpoint, Dispatch, EndReason, GatewayEnd, GatewayError, NativeDiscordJournal,
    MESSAGE_INTENTS, REACTION_INTENTS,
};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod activity_tests;
#[cfg(test)]
mod journal_fixture;
#[cfg(test)]
mod journal_tests;

#[cfg(test)]
mod dispatch_fixture;
#[cfg(test)]
mod inbox_dispatch_tests;
#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod normalization_tests;
#[cfg(test)]
mod quote_tests;
#[cfg(test)]
pub(crate) mod rest_fixture;
#[cfg(test)]
mod rest_tests;

#[cfg(test)]
mod routed_execution_tests;
#[cfg(test)]
mod thread_execution_tests;
#[cfg(test)]
mod thread_tests;
pub(crate) fn native_source_revision(
    definition: &cmaeg_core::channel::ChannelDefinition,
) -> Option<String> {
    journal_model::Authority::from_definition(definition)
        .ok()
        .map(|authority| authority.revision)
}

#[cfg(test)]
mod message_event_tests;

#[cfg(test)]
mod message_event_execution_tests;

#[cfg(test)]
pub(crate) mod reaction_tests;

#[cfg(test)]
mod reaction_execution_tests;
