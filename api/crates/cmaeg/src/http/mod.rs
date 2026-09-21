//! HTTP projections for CMAEG management operations.

pub mod management;
mod management_responses;
pub mod problems;

pub use crate::channels::management::ManagementState;
pub use management::{management_router, ChannelResponse};
