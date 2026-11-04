//! HTTP projections for CMAEG management operations.

pub mod activity;
pub mod callbacks;
pub mod channel_nodes;
mod chat_dispatch;
pub(crate) mod connection_browser;
pub mod connections;
pub(crate) mod connections_ui;
pub mod creation;
pub(crate) mod docs;
pub mod event_attestation;
pub mod events;
pub mod form_preparation;

pub mod history;
pub(crate) mod html;
pub mod management;
mod management_responses;
pub mod media;
pub mod media_responses;
pub mod message_effects;
pub mod metrics;
pub mod native_lark;
pub mod outbound;
pub mod partitions;
pub mod problems;
pub(crate) mod public_urls;
pub mod readiness;
pub mod relay_capabilities;
pub mod relay_resource_connections;
pub mod reply_targets;
pub mod schema;
pub mod telegram;
pub mod threads;
pub mod tools;
pub mod whatsapp;

pub use crate::channels::management::ManagementState;
pub use management::{management_router, ChannelResponse};
