//! Durable, actor-authorized Bot configuration and explicit creation consent.
mod helpers;
mod model;
mod ports;
mod service;
mod view;
pub use model::*;
pub use ports::*;
pub use service::*;

#[cfg(test)]
mod tests;
