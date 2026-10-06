//! Neutral delivery intents; the transport engine owns external sends.
pub mod custom;
pub mod engine;
mod event;
pub mod intents;
mod pacing;
pub mod relay_client;
pub mod render;
mod send;
pub mod sync;
mod telegram;
pub mod webhook_client;
mod worker;
