//! Sandbox Manager owns environment authority independently of Agent Manager.
//!
//! No Agent, turn, profile, or conversation persistence is reachable from this
//! crate. A caller's opaque reference is correlation data, never authorization.

pub mod auth;
pub mod model;
pub mod permissions;
pub mod schema;
pub mod store;
