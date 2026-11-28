//! Immutable completion evidence, implemented in the Agent-log library.
pub(in crate::store) use cma_am_agent_log::publication::*;

#[cfg(test)]
use super::{digest, MAX_SEQUENCE};
#[cfg(test)]
use mongodb::bson;

#[cfg(test)]
#[path = "writer_receipt_tests.rs"]
mod tests;
