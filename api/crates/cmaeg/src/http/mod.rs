//! HTTP projections for CMAEG management operations.

pub mod callbacks;
mod chat_dispatch;
pub(crate) mod connection_browser;
pub mod connections;
pub(crate) mod connections_ui;
pub mod creation;
pub(crate) mod docs;
pub mod events;
pub(crate) mod html;
pub mod management;
mod management_responses;
pub mod media;
pub mod metrics;
pub mod problems;
pub mod readiness;
pub mod reply_targets;
#[doc(hidden)]
pub mod request_start;
pub mod schema;
pub mod telegram;
pub mod tools;

pub use crate::channels::management::ManagementState;
pub use management::{management_router, ChannelResponse};
