mod admission;
pub use admission::{Admission, AdmissionBudget, AdmissionRequest, CONTROL, JOBS};
mod chart_encoding;
mod charts;
mod control;
mod controlled_updates;
mod owners;
mod reports;
mod rows;

pub use charts::{chart_documents, read_series};
pub use control::fence as control_fence;
pub use owners::{read_owner_page, validate_owner_page};
pub use reports::report_membership_pipeline;
pub use rows::{read_row, read_row_page};

use layer2_core::sandbox_metrics::{MetricsError, Result};
use mongodb::{
    bson::{doc, Document},
    Collection,
};
use serde::{de::DeserializeOwned, Serialize};
use std::time::Duration;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn unavailable(error: impl std::fmt::Display) -> MetricsError {
    tracing::warn!(target: "cma::store::usage_reports", error=%error,"reporting store unavailable");
    MetricsError::Unavailable
}
pub fn encode<T: Serialize>(record: &T) -> Result<mongodb::bson::Bson> {
    mongodb::bson::serialize_to_bson(record).map_err(unavailable)
}
pub fn decode<T: DeserializeOwned>(document: &Document) -> Result<T> {
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
pub fn expires(at: u64) -> mongodb::bson::DateTime {
    mongodb::bson::DateTime::from_millis((at * 1000) as i64)
}
