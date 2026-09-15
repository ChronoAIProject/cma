//! Signed NyxID callback acceptance. A verified callback grants only permission
//! to persist an inbox entry; deferred work requires separate current authority.
mod jwt;
mod model;
mod payload;
mod service;
mod timestamp;

pub use jwt::{CallbackKeySet, CallbackTokenPolicy};
pub use model::*;
pub use service::{CallbackHeaders, CallbackVerifier};

#[cfg(test)]
mod tests;
