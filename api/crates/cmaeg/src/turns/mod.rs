//! Bounded, stateless provider turn execution and drain.
mod audience;
pub mod capacity;
mod consume;
mod metadata;
pub mod outcomes;
mod projection;
pub mod render_input;
pub mod render_media;
pub mod runner;
pub mod sequencer;
pub mod tracker;
pub use runner::TurnRunner;
pub use tracker::TurnTracker;
