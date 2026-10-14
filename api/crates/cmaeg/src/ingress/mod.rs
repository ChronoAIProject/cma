//! Stateless event verification, admission and launch handoff.
pub mod admission;
pub mod custom;
pub mod launch;
pub(crate) mod media_guard;
pub mod receipts;
pub mod relay;
pub mod telegram;

mod state;
pub use state::IngressState;
