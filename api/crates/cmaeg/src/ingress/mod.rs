//! Stateless event verification, admission and launch handoff.
pub mod admission;
pub mod custom;
pub mod launch;
pub mod relay;

mod state;
pub use state::IngressState;
