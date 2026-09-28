//! HTTP projections for CMAEG management operations.

pub mod callbacks;
pub mod creation;
pub(crate) mod docs;
pub mod events;
pub mod management;
mod management_responses;
pub mod metrics;
pub mod problems;
pub mod readiness;
pub mod reply_targets;
pub mod schema;
pub mod tools;

pub use crate::channels::management::ManagementState;
pub use management::{management_router, ChannelResponse};
