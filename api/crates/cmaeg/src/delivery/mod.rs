//! Neutral delivery intents; the transport engine owns external sends.
mod address;
pub mod custom;
mod default_destination;
pub mod engine;
mod event;
mod failure;
pub(crate) use failure::provider_code;
pub mod gate;
mod group;
pub mod intents;
mod label;
pub mod notice;
mod pacing;
pub mod part_keys;
pub mod relay_client;
pub mod render;
pub mod router;
mod send;
mod step;
pub mod sync;
mod telegram;
mod tool_receipt;
pub mod webhook_client;
mod worker;

pub(crate) use address::{default_ok, routing_send, routing_send_for_message, SendKind};
