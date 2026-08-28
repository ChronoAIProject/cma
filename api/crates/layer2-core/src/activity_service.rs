//! Keeping, and reading back, the record of what the service was asked to do.
//!
//! Two halves with deliberately different shapes. Recording is something the
//! service does to itself: there is no caller to authorise, it must never fail
//! the work it is describing, and it happens on every request. Reading is an
//! operator action, gated on a capability nobody holds by accident.
//!
//! **Reading is not scoped to the reader.** There is no path by which an
//! ordinary caller reads their own entries, and that is a decision rather than
//! an omission: entries name other subjects as targets, the shape of the log
//! tells a reader what the service is doing and for whom, and a self-serve
//! filter is an access-control surface that would have to stay correct forever
//! to be worth the convenience. Somebody who wants to know what happened to
//! their own session reads that session's event log, which is theirs.

use std::sync::Arc;

use crate::activity::{ActivityQuery, ActivityRecord};
use crate::authz::{Caller, Capability};
use crate::error::CoreError;
use crate::store::{ActivityStore, StoreError};

/// The record of what the service was asked to do.
pub struct ActivityService {
    store: Arc<dyn ActivityStore>,
}

impl ActivityService {
    /// Build the service over its port.
    pub fn new(store: Arc<dyn ActivityStore>) -> Arc<Self> {
        Arc::new(Self { store })
    }

    /// Keep one entry.
    ///
    /// Takes no caller and returns nothing, because there is nothing sensible
    /// for the work being described to do about a failure here. A request that
    /// succeeded did succeed, and failing it because the account of it could
    /// not be written would turn a storage problem into an outage.
    ///
    /// A failure is therefore reported at error level with everything needed to
    /// notice it — never silently dropped — so a log that has stopped being
    /// written is visible in the process output rather than only discovered
    /// when somebody goes looking for an entry that was never kept.
    pub async fn record(&self, record: ActivityRecord) -> bool {
        let actor = record
            .actor
            .as_ref()
            .map(|s| s.as_str())
            .unwrap_or("<none>");
        let target = record.target.as_ref().map(|t| t.id()).unwrap_or("<none>");
        if let Err(e) = self.store.append(&record).await {
            tracing::error!(
                error = %e,
                action = ?record.action,
                outcome = ?record.outcome,
                actor,
                target,
                "an action could not be recorded"
            );
            return false;
        }
        if record.outcome.is_notable() {
            // Refusals and failures are worth a line in the process log too:
            // they are what somebody grepping for a problem looks for first,
            // and they are rare enough not to be noise.
            tracing::info!(
                action = ?record.action,
                outcome = ?record.outcome,
                actor,
                target,
                "an action was refused or failed"
            );
        }
        true
    }

    /// Read the most recent entries, newest first.
    pub async fn list(
        &self,
        caller: &Caller,
        query: ActivityQuery,
    ) -> Result<Vec<ActivityRecord>, CoreError> {
        caller.authorization.require(Capability::ReadActivityLog)?;
        let query = query.clamped();
        let entries = self.store.recent(&query).await.map_err(store_failed)?;
        tracing::info!(
            reader = %caller.subject,
            filtered_to = query.actor.as_ref().map(|s| s.as_str()).unwrap_or("<everyone>"),
            returned = entries.len(),
            "the record of actions was read"
        );
        Ok(entries)
    }
}

/// Render a storage failure as a core error.
///
/// Unreachable storage is reported as unavailability rather than as an empty
/// log: a reader told there is nothing to see would conclude nothing happened,
/// which is the one wrong answer this surface can give.
fn store_failed(e: StoreError) -> CoreError {
    CoreError::RuntimeUnavailable(format!("activity store: {e}"))
}

#[cfg(test)]
#[path = "activity_service_tests.rs"]
mod tests;
