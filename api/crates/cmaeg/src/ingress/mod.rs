//! Stateless event verification, admission and launch handoff.
pub mod admission;
pub mod custom;
pub mod launch;
pub(crate) mod media_guard;
pub(crate) mod native_interaction;
mod native_mentions;
pub mod receipts;
pub mod relay;
pub mod routed;
pub mod routing_plan;
pub mod routing_store;
pub mod telegram;

mod state;
pub use state::IngressState;
