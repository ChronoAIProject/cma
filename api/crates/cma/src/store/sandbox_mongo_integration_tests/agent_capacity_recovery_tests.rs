//! S5 real-Mongo contracts. Model stubs are tested separately in layer2-core.
use super::*;

mod composition;
mod discovery;
mod fixture;
mod observations;
mod pacing;
mod records;
mod restart;
mod segmented_admission;
mod service;
mod source;
mod transactions;
mod worker;

pub(super) use fixture::ready_with_created;
