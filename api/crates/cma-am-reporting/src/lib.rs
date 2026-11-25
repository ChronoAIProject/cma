//! Persistence for Agent Manager's bounded reporting read model.

mod store {
    pub mod allocation_buckets;
    pub mod mongo;
    pub mod transaction_scope;
    pub(super) mod usage_reports;
}

pub use store::usage_reports::{
    build_chart_totals, build_owner_chart_projections, build_ranked_projections, chart_documents,
    control_fence, decode, encode, expires, now, optional_query_error, read_chart_projection,
    read_owner_page, read_row, read_row_page, read_series, report_membership_pipeline, unavailable,
    validate_owner_page, Admission, AdmissionBudget, AdmissionRequest, ProjectionBatch,
    ProjectionManifest, TotalProjection, CONTROL, JOBS,
};

pub mod metric_bindings;
pub mod metric_rollups;
pub mod pool;
pub mod usage_rollups;

pub use store::{allocation_buckets, mongo::ReportingStore, transaction_scope};

#[cfg(feature = "test-support")]
pub use store::usage_reports::read_reference_series;

#[cfg(feature = "test-support")]
pub use store::usage_reports::benchmark_ranked_projections;
