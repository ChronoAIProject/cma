//! CMA's opaque channel-record transport.

mod client;
pub mod import;
mod kind;
pub mod mongo;

pub use client::{RecordClient, RecordError, RecordHead, RecordValue};
pub use kind::RecordKind;
