//! Measurements of the disposable fixture process and its Mongo driver pools.
use mongodb::{event::cmap::CmapEvent, Client};
use serde::Serialize;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct PoolUsage {
    active: HashMap<u64, Instant>,
    started: Instant,
    occupied: Duration,
    peak: usize,
    limit: u32,
    failed: u64,
}

impl Default for PoolUsage {
    fn default() -> Self {
        Self {
            active: HashMap::new(),
            started: Instant::now(),
            occupied: Duration::ZERO,
            peak: 0,
            limit: 10,
            failed: 0,
        }
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct PoolReport {
    pub configured_limit_per_server: u32,
    pub peak_checked_out: usize,
    pub checked_out_at_end: usize,
    pub occupied_us: u128,
    pub window_us: u128,
    pub checkout_failures: u64,
}

impl PoolUsage {
    pub fn set_limit(&mut self, limit: Option<u32>) {
        self.limit = limit.unwrap_or(10);
    }

    pub fn reset(&mut self) {
        self.reset_at(Instant::now());
    }

    fn reset_at(&mut self, now: Instant) {
        // A connection checked out before the sampling boundary may remain in
        // use through it. Keep the live set and charge only the new interval.
        self.started = now;
        self.occupied = Duration::ZERO;
        self.peak = self.active.len();
        self.failed = 0;
        for started in self.active.values_mut() {
            *started = now;
        }
    }

    fn checkout(&mut self, key: u64, now: Instant) {
        assert!(
            self.active.insert(key, now).is_none(),
            "duplicate checkout event"
        );
        self.peak = self.peak.max(self.active.len());
    }

    fn returned(&mut self, key: u64, now: Instant) {
        if let Some(started) = self.active.remove(&key) {
            self.occupied += now.duration_since(started);
        }
    }

    pub fn observe(&mut self, event: &CmapEvent) {
        use crate::store::mongo_latency::connection_key;
        let now = Instant::now();
        match event {
            CmapEvent::ConnectionCheckedOut(event) => {
                self.checkout(connection_key(&event.address, event.connection_id), now)
            }
            CmapEvent::ConnectionCheckedIn(event) => {
                self.returned(connection_key(&event.address, event.connection_id), now)
            }
            CmapEvent::ConnectionClosed(event) => {
                self.returned(connection_key(&event.address, event.connection_id), now)
            }
            CmapEvent::ConnectionCheckoutFailed(_) => self.failed += 1,
            _ => {}
        }
    }

    pub fn report(&self) -> PoolReport {
        self.report_at(Instant::now())
    }

    fn report_at(&self, now: Instant) -> PoolReport {
        let occupied = self.occupied
            + self
                .active
                .values()
                .map(|started| now.duration_since(*started))
                .sum::<Duration>();
        PoolReport {
            configured_limit_per_server: self.limit,
            peak_checked_out: self.peak,
            checked_out_at_end: self.active.len(),
            occupied_us: occupied.as_micros(),
            window_us: now.duration_since(self.started).as_micros(),
            checkout_failures: self.failed,
        }
    }
}

pub(crate) fn process_cpu() -> Duration {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the supplied rusage on success. RUSAGE_SELF
    // measures only this process, including its serving and fixture threads.
    assert_eq!(
        unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) },
        0
    );
    let usage = unsafe { usage.assume_init() };
    let duration = |time: libc::timeval| {
        Duration::from_secs(time.tv_sec.try_into().unwrap())
            + Duration::from_micros(time.tv_usec.try_into().unwrap())
    };
    duration(usage.ru_utime) + duration(usage.ru_stime)
}

/// The idle fixture has a dedicated Mongo replica set. Server-wide examined
/// document counts avoid changing query load with per-command profiling.
pub(crate) async fn server_examined_documents(client: &Client) -> u64 {
    let result = client
        .database("admin")
        .run_command(mongodb::bson::doc! {"serverStatus":1})
        .await
        .unwrap();
    let executor = result
        .get_document("metrics")
        .unwrap()
        .get_document("queryExecutor")
        .unwrap();
    executor
        .get_i64("scannedObjects")
        .or_else(|_| executor.get_i32("scannedObjects").map(i64::from))
        .unwrap()
        .try_into()
        .unwrap()
}

#[test]
fn pool_window_counts_connections_crossing_both_boundaries() {
    let start = Instant::now();
    let mut pool = PoolUsage::default();
    pool.checkout(1, start);
    pool.reset_at(start + Duration::from_millis(1));
    pool.checkout(2, start + Duration::from_millis(2));
    pool.returned(1, start + Duration::from_millis(4));
    let report = pool.report_at(start + Duration::from_millis(6));
    assert_eq!(report.window_us, 5_000);
    assert_eq!(report.occupied_us, 7_000);
    assert_eq!(report.peak_checked_out, 2);
    assert_eq!(report.checked_out_at_end, 1);
}
