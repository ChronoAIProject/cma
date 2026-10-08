//! Fixtures shared by the console's unit tests.
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

use cmaeg_core::ids::RecordId;
use futures_util::future::BoxFuture;

use super::{
    authority::{Authorized, OwnerScope},
    cma_budget::RecordHeads,
};
use crate::{
    auth::Creator,
    records::{RecordError, RecordHead},
};

/// A Personal authority for `subject`, as `ConsoleAuthority` derives it.
pub(crate) fn authorized(subject: &str) -> Authorized {
    Authorized::new(
        Creator {
            subject: subject.to_owned(),
            token_hash: "sha256:fixture".to_owned(),
            organization: false,
        },
        OwnerScope::Personal,
    )
}

/// The `index`-th fixture record id, a canonical UUID v4 that sorts by index.
pub(crate) fn record(index: usize) -> RecordId {
    RecordId::new(format!("00000000-0000-4000-8000-{index:012}")).unwrap()
}

/// CMA's record listing, recording when each request started and ended and
/// how many overlapped.
#[derive(Default)]
pub(crate) struct FakeHeads {
    pub heads: Mutex<Vec<RecordHead>>,
    pub calls: Mutex<Vec<(Option<String>, Instant, Instant)>>,
    pub inflight: AtomicUsize,
    pub max_inflight: AtomicUsize,
    pub delay: Duration,
    pub fail: Mutex<bool>,
}

impl FakeHeads {
    /// `count` records alternating between owners `alice` and `bob`.
    pub(crate) fn seeded(count: usize) -> Self {
        let heads = (0..count)
            .map(|index| RecordHead {
                record_id: record(index).to_string(),
                owner: if index % 2 == 0 { "alice" } else { "bob" }.to_owned(),
                version: 1,
            })
            .collect();
        Self {
            heads: Mutex::new(heads),
            delay: Duration::from_millis(5),
            ..Self::default()
        }
    }
}

impl RecordHeads for FakeHeads {
    fn list<'a>(
        &'a self,
        owner: Option<&'a str>,
        after: Option<&'a str>,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<RecordHead>, RecordError>> {
        Box::pin(async move {
            let started = Instant::now();
            let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_inflight.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(self.delay).await;
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            self.calls
                .lock()
                .unwrap()
                .push((owner.map(str::to_owned), started, Instant::now()));
            if *self.fail.lock().unwrap() {
                return Err(RecordError::Unavailable("fixture outage".into()));
            }
            let heads = self.heads.lock().unwrap();
            Ok(heads
                .iter()
                .filter(|head| owner.is_none_or(|owner| head.owner == owner))
                .filter(|head| after.is_none_or(|after| head.record_id.as_str() > after))
                .take(limit)
                .cloned()
                .collect())
        })
    }
}
