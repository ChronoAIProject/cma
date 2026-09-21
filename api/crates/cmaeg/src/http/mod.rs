//! HTTP projections for CMAEG management operations.

pub mod callbacks;
pub mod events;
pub mod management;
mod management_responses;
pub mod problems;
pub mod readiness;
pub mod tools;

pub use crate::channels::management::ManagementState;
pub use management::{management_router, ChannelResponse};
