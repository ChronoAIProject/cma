mod admission;
pub use admission::{Admission, AdmissionBudget, AdmissionRequest, CONTROL, JOBS};
mod chart_encoding;
mod chart_projections;
mod charts;
mod control;
mod controlled_updates;
mod owners;
mod projection_codec;
mod projection_manifest;
pub use projection_manifest::ProjectionManifest;
mod projection_reads;
mod ranked_buckets;
mod reports;
mod rows;
mod total_projection;

pub use chart_projections::{
    build_chart_totals, build_owner_chart_projections, build_ranked_projections, ProjectionBatch,
};
pub use charts::{chart_documents, read_series};
pub use control::fence as control_fence;
pub use owners::{read_owner_page, validate_owner_page};
pub use projection_reads::read_chart_projection;
pub use reports::report_membership_pipeline;
pub use rows::{read_row, read_row_page};
pub use total_projection::TotalProjection;

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
/// Optional read work may be omitted after a definite server query timeout.
/// Keep all other driver failures distinct from an exhausted work budget.
pub fn optional_query_error(error: mongodb::error::Error) -> MetricsError {
    if matches!(error.kind.as_ref(), mongodb::error::ErrorKind::Command(command) if command.code == 50)
    {
        MetricsError::Busy
    } else {
        MetricsError::Unavailable
    }
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

#[cfg(feature = "test-support")]
pub use charts::read_reference_series;

#[cfg(feature = "test-support")]
pub use chart_projections::benchmark_ranked_projections;
