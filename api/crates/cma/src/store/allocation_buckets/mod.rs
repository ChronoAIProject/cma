//! Disposable daily/hourly projections; the allocated ledger remains authoritative.
mod materialize;
mod publication;
mod reads;
mod schema;
mod worker;
pub(crate) use worker::Lane;

use super::usage_reports::{ReportingStore, CONTROL, DB_DEADLINE};
pub(crate) use cma_am_reporting::allocation_buckets::COLLECTION;
use cma_am_reporting::allocation_buckets::{DayBucket, DAY, HOUR};
use layer2_core::{
    sandbox_metrics::{allocation, MetricsError, Range, Result},
    sandbox_usage::SandboxUsage,
};
use mongodb::bson::{doc, Document};
use serde::{Deserialize, Serialize};

#[cfg(test)]
use layer2_core::sandbox_usage::Allocation;

// Fixed outcomes only: driver errors can include collection filters or identities.
fn unavailable(_: impl std::fmt::Display) -> MetricsError {
    MetricsError::Unavailable
}

fn encode<T: Serialize>(value: &T) -> Result<mongodb::bson::Bson> {
    mongodb::bson::serialize_to_bson(value).map_err(unavailable)
}

fn decode<T: serde::de::DeserializeOwned>(document: &Document) -> Result<T> {
    if document.get_i64("document_schema_version").ok() != Some(1) {
        return Err(MetricsError::Unavailable);
    }
    mongodb::bson::deserialize_from_bson(
        document
            .get("record")
            .cloned()
            .ok_or(MetricsError::Unavailable)?,
    )
    .map_err(unavailable)
}

#[cfg(test)]
mod read_fixture;
#[cfg(test)]
mod tests;
