//! Durable ingress enumeration and live summaries, independent of the recent view.
pub mod admin;
mod backfill;
pub use crate::config::activity_history as config;
mod cursor;
mod model;
mod mongo;
mod mongo_insert;
mod paging;
mod payload;
pub mod repository;
pub mod schema;
mod service;
pub use config::Config;
pub use model::*;
pub use mongo::MongoHistory;
pub use service::ActivityHistory;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConfiguration,
    Disabled,
    UnsupportedBackend,
    InvalidRequest,
    Expired,
    Unreadable,
    Unavailable,
    DetailLimit,
    NotFound,
}
fn digest(parts: &[&str]) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(parts).expect("string tuple"))
    )
}
fn scope(owner: &str, record: &str) -> String {
    digest(&["activity-history-v1", owner, record])
}
fn event_key(scope: &str, event: &str) -> String {
    digest(&["activity-history-event-v1", scope, event])
}
fn now_ms() -> i64 {
    (time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000) as i64
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => "invalid activity history configuration",
            Self::Disabled => "activity history is not enabled",
            Self::UnsupportedBackend => "activity history requires the indexed gateway backend",
            Self::InvalidRequest => "invalid activity history request or cursor",
            Self::Expired => "activity history paging session expired; restart the range and deduplicate event identities",
            Self::Unreadable => "activity history record is unreadable",
            Self::Unavailable => "activity history storage is unavailable",
            Self::DetailLimit => "activity history detail exceeds its bounded storage allowance",
            Self::NotFound => "activity history event was not found",
        })
    }
}
impl std::error::Error for Error {}

/// Compose only after storage has selected its authoritative backend.
pub(crate) async fn configure(
    state: &mut crate::channels::management::ManagementState,
) -> Result<(), Error> {
    let config = Config::from_env()?;
    if !config.enabled {
        return Ok(());
    }
    let repository = match state.records.database() {
        Some(db) => {
            schema::verify_grants(&db).await?;
            Some(std::sync::Arc::new(MongoHistory::open(db).await?)
                as std::sync::Arc<dyn repository::ActivityHistoryRepository>)
        }
        None if config.capture => return Err(Error::UnsupportedBackend),
        None => None,
    };
    let history = std::sync::Arc::new(ActivityHistory::new(
        repository,
        state.master_keys.clone(),
        config,
    ));
    state.activity = std::sync::Arc::new((*state.activity).clone().with_history(history));
    Ok(())
}

impl super::ActivityStore {
    pub fn with_history(mut self, history: std::sync::Arc<ActivityHistory>) -> Self {
        self.history = history;
        self
    }
}

#[cfg(test)]
pub(crate) mod test_repository;
#[cfg(test)]
mod tests;

#[cfg(test)]
mod detail_tests;

#[cfg(test)]
mod mongo_tests;

#[cfg(test)]
mod coverage_tests;
