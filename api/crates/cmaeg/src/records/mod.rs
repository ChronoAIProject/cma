//! CMA's opaque channel-record transport.

mod client;
pub mod import;
pub mod mongo;

pub use client::{RecordClient, RecordError, RecordHead, RecordValue};
