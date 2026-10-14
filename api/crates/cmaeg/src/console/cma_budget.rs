//! The console's share of CMA: every console read of channel records goes
//! through four permits of its own, apart from the resolver's cold-start
//! restores and their 800 ms budget, and record heads are listed one paced
//! page at a time.
use std::time::Duration;

use futures_util::future::BoxFuture;
use ring::rand::{SecureRandom, SystemRandom};
use tokio::sync::{Semaphore, SemaphorePermit};

use crate::records::{RecordClient, RecordError, RecordHead};

/// Concurrent console reads of CMA.
pub const PERMITS: usize = 4;
/// Record heads per listing page.
pub const PAGE: usize = 100;

/// The console's CMA permits.
pub struct CmaBudget {
    permits: Semaphore,
}

impl Default for CmaBudget {
    fn default() -> Self {
        Self {
            permits: Semaphore::new(PERMITS),
        }
    }
}

impl CmaBudget {
    /// One of the four permits; held for exactly one CMA request.
    pub async fn permit(&self) -> SemaphorePermit<'_> {
        // The semaphore is never closed, so acquiring cannot fail.
        self.permits
            .acquire()
            .await
            .expect("the console's CMA semaphore is never closed")
    }
}

/// Where record heads come from: CMA's listing, or a fake in tests.
pub trait RecordHeads: Send + Sync {
    fn list<'a>(
        &'a self,
        owner: Option<&'a str>,
        after: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<RecordHead>, RecordError>>;
}

impl RecordHeads for RecordClient {
    fn list<'a>(
        &'a self,
        owner: Option<&'a str>,
        after: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<RecordHead>, RecordError>> {
        Box::pin(RecordClient::list(self, owner, after, limit))
    }
}

/// The pause between listing pages: `base` ± half of it.
#[derive(Debug, Clone, Copy)]
pub struct Pacing {
    pub base: Duration,
}

impl Default for Pacing {
    fn default() -> Self {
        Self {
            base: Duration::from_millis(100),
        }
    }
}

impl Pacing {
    /// A uniformly jittered pause from `base / 2` to `base * 3 / 2`.
    fn pause(&self) -> Duration {
        let mut bytes = [0; 8];
        let ratio = match SystemRandom::new().fill(&mut bytes) {
            Ok(()) => u64::from_le_bytes(bytes) as f64 / u64::MAX as f64,
            Err(_) => 0.5,
        };
        self.base.mul_f64(0.5 + ratio)
    }
}

/// List every record head (of one owner, when `listed_owner` is set),
/// strictly one page at a time, each page under one permit, with a paced
/// pause between pages. `page` sees each page as it arrives. Returns how
/// many heads were listed.
pub async fn page_all(
    heads: &dyn RecordHeads,
    budget: &CmaBudget,
    pacing: Pacing,
    listed_owner: Option<&str>,
    mut page: impl FnMut(&[RecordHead]),
) -> Result<u64, RecordError> {
    let mut after: Option<String> = None;
    let mut listed = 0u64;
    loop {
        let items = {
            let _permit = budget.permit().await;
            heads.list(listed_owner, after.as_deref(), PAGE).await?
        };
        listed += items.len() as u64;
        page(&items);
        if items.len() < PAGE {
            return Ok(listed);
        }
        after = items.last().map(|head| head.record_id.clone());
        tokio::time::sleep(pacing.pause()).await;
    }
}

#[cfg(test)]
#[path = "cma_budget_tests.rs"]
mod tests;
