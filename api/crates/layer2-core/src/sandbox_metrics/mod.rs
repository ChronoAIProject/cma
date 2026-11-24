//! Observational resource reports. These quantities never authorize billing.
pub mod aggregate;
pub mod allocation;
pub mod cursor;
pub mod initial;
pub mod measured_evidence;
pub mod model;
pub mod options;
pub mod owner_allocation;
pub mod query;
pub mod series;
pub mod service;
pub mod view;

pub use initial::*;
pub use model::*;
pub use options::*;
pub use owner_allocation::*;
pub use query::*;
pub use service::*;
pub use view::*;

pub const RETENTION_SECONDS: u64 = 30 * 86_400;
pub const IDENTITY_RETENTION_SECONDS: u64 = 90 * 86_400;
pub const FRESHNESS_SECONDS: u64 = 90;
pub const FINALIZATION_DELAY_SECONDS: u64 = 120;
pub const REPORT_TTL_SECONDS: u64 = 600;
pub const SOURCE: &str = "victoriametrics:kubelet-resource-and-volume:v1";
pub const ALGORITHM_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum MetricsError {
    #[error("retained usage views are not enabled")]
    ViewDisabled,
    #[error("{0}")]
    Invalid(String),
    #[error("usage authority required")]
    Forbidden,
    #[error("usage report or resource not found")]
    NotFound,
    #[error("usage report expired; refresh the report")]
    Expired,
    #[error("reporting budget exhausted; retry with a narrower range")]
    Busy,
    #[error("usage reporting is temporarily unavailable; retry or use a narrower range")]
    Unavailable,
}
pub type Result<T> = std::result::Result<T, MetricsError>;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod options_tests;

#[cfg(test)]
mod options_regression_tests;

#[cfg(test)]
mod series_tests;

#[cfg(test)]
mod view_tests;

#[cfg(test)]
mod initial_tests;
