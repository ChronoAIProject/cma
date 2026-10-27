//! Stateless event verification, admission and launch handoff.
pub mod admission;
pub mod custom;
pub mod launch;
pub(crate) mod media_guard;
pub(crate) mod message_event;
pub(crate) mod native_interaction;
mod native_mentions;
pub mod receipts;
pub mod relay;
pub mod routed;
pub mod routing_plan;
pub mod routing_store;
pub mod telegram;
pub(crate) mod whatsapp;

mod state;
pub use state::IngressState;

#[cfg(test)]
mod whatsapp_tests;

#[cfg(test)]
mod native_lark_tests;

#[cfg(test)]
mod whatsapp_control_tests;

#[cfg(test)]
mod native_lark_control_tests;
