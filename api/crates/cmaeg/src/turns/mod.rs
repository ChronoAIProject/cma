//! Bounded, stateless provider turn execution and drain.
mod consume;
mod metadata;
pub mod outcomes;
mod projection;
pub mod render_input;
pub mod runner;
pub mod tracker;
pub use runner::TurnRunner;
pub use tracker::TurnTracker;
