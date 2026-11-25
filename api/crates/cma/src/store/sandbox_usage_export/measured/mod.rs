mod bridge;
mod cli;
mod delivery;
mod discovery;
mod freeze;
mod keys;
mod lease;
mod payload;
mod prepare;
mod publish;
mod records;
mod repair;
mod replay;
mod transaction;
pub(crate) use bridge::retain_binding_bridge;
pub(crate) use cli::command as repair_command;
pub(crate) use prepare::MeasuredInput;

use super::records::{decode, encode, hash, number, Captured, ResourceSnapshot, Stream};
use super::{CaptureStore, CAPTURES, EVIDENCE, INVENTORY, OUTBOX, STREAMS};
use crate::store::sandbox_events::TransactionEventError;
use layer2_core::sandbox::store::SandboxStoreError;
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};
use records::*;

fn corrupt() -> SandboxStoreError {
    SandboxStoreError::Corrupt("measured usage evidence integrity conflict".into())
}

fn unavailable(_: impl std::fmt::Display) -> SandboxStoreError {
    SandboxStoreError::Unavailable("measured usage evidence unavailable".into())
}

#[cfg(test)]
mod mongo_tests;
