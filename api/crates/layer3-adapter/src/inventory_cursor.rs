//! Process-local discovery checkpoints. Only a complete inventory is exposed.
use crate::{AdapterError, Result};
use std::{future::Future, time::Duration};
use tokio::time::Instant;

/// One cancellation-safe discovery step. Implementations commit successful
/// reads before awaiting more work and expose no partial deletion authority.
#[async_trait::async_trait]
pub trait InventoryCursor<T>: Send + Sync {
    async fn advance(&mut self) -> Result<Option<Vec<T>>>;
}

#[derive(Clone, Copy)]
pub(crate) enum Child {
    RouteList,
    ClaimRead,
    AttachmentRead,
}

impl Child {
    fn name(self) -> &'static str {
        match self {
            Self::RouteList => "route_list",
            Self::ClaimRead => "claim_read",
            Self::AttachmentRead => "attachment_read",
        }
    }
}

const DEADLINE: Duration = Duration::from_secs(5);

pub(crate) async fn request<T>(child: Child, work: impl Future<Output = Result<T>>) -> Result<T> {
    let mut timing = Timing {
        child,
        started: Instant::now(),
        outcome: "cancelled",
    };
    match tokio::time::timeout(DEADLINE, work).await {
        Ok(result) => {
            timing.outcome = if result.is_ok() {
                "complete"
            } else {
                "unavailable"
            };
            result
        }
        Err(_) => {
            timing.outcome = "deadline";
            Err(AdapterError::Unavailable(
                "inventory child deadline exceeded".into(),
            ))
        }
    }
}

struct Timing {
    child: Child,
    started: Instant,
    outcome: &'static str,
}

impl Drop for Timing {
    fn drop(&mut self) {
        // An enclosing operation deadline can cancel at the same instant as
        // the child's timer. Report that boundary as a deadline as well.
        let outcome = if self.outcome == "cancelled" && self.started.elapsed() >= DEADLINE {
            "deadline"
        } else {
            self.outcome
        };
        tracing::info!(parent: None,
            child = self.child.name(), outcome, count = 1_u64,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            "sandbox maintenance inventory child completed"
        );
    }
}

#[cfg(test)]
#[path = "inventory_cursor_tests.rs"]
mod tests;
