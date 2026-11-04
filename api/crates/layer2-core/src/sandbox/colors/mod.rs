//! Sandbox colours: every Agent in one Sandbox wears that Sandbox's colour, so
//! an owner can tell at a glance which Sandbox an Agent works in.
//!
//! Colours live beside the Sandbox aggregate rather than in it. They are owner
//! presentation, never configuration, and keeping them out of the strictly
//! read Sandbox record lets a rolled-back reader keep reading every Sandbox.

mod color;
pub mod palette;
mod service;
mod store;

pub use color::{InvalidSandboxColor, SandboxColor};
pub use palette::{ColorRandom, PaletteError, SystemColorRandom};
pub use service::SandboxColorService;
pub use store::{InMemorySandboxColorStore, OwnerSandboxColors, SandboxColorStore};

#[cfg(test)]
mod tests;
