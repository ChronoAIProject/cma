//! CMA's opaque channel-record transport.

mod client;
pub mod import;
mod import_gate;
mod kind;
pub mod mongo;
mod retention;

pub use client::{RecordClient, RecordError, RecordHead, RecordValue};
pub use import_gate::{
    ImportGate, GITHUB_IMPORT_DISABLED, GITHUB_IMPORT_KEYS_REQUIRED, GITHUB_IMPORT_UNREADABLE,
};
pub use kind::RecordKind;
