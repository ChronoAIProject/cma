use super::*;
use std::sync::atomic::AtomicUsize;
use tokio::time::Instant;

#[derive(Default)]
pub(super) struct ObservationTrace {
    starts: Mutex<Vec<Instant>>,
    delay: Mutex<Duration>,
    active: AtomicUsize,
    peak: AtomicUsize,
}

pub(super) struct ObservationRead<'a>(&'a ObservationTrace);

impl ObservationTrace {
    pub(super) fn set_delay(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }
    pub(super) fn start(&self) -> ObservationRead<'_> {
        self.starts.lock().unwrap().push(Instant::now());
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        ObservationRead(self)
    }
    pub(super) async fn respond(&self) {
        let delay = *self.delay.lock().unwrap();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
}

impl Drop for ObservationRead<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

fn calls(pools: &Pools, operation: &str) -> usize {
    pools
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|call| **call == operation)
        .count()
}

fn assert_serial(trace: &ObservationTrace) {
    assert_eq!(trace.active.load(Ordering::SeqCst), 0);
    assert_eq!(trace.peak.load(Ordering::SeqCst), 1);
    let starts = trace.starts.lock().unwrap();
    assert!(starts
        .windows(2)
        .all(|pair| pair[1] - pair[0] >= Duration::from_millis(250)));
}

#[tokio::test(start_paused = true)]
async fn claim_cadence_delayed_assignment_measurement() {
    let mut elapsed_ms = Vec::new();
    let mut counts = Vec::new();
    // A complete 350 ms phase interval avoids selecting only favorable offsets.
    for ready_ms in (501..=841).step_by(10) {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        pools.pending_claim.store(true, Ordering::SeqCst);
        *pools.observation.delay.lock().unwrap() = Duration::from_millis(100);
        let worker = worker(&h, pools.clone());
        let started = Instant::now();
        let assign = async {
            tokio::time::sleep(Duration::from_millis(ready_ms)).await;
            pools.pending_claim.store(false, Ordering::SeqCst);
        };
        let (result, ()) = tokio::join!(worker.run_one(&created.id, start_time(&created)), assign);
        assert!(result.unwrap());
        let stored = h.stored(&created.id).await;
        assert_eq!(stored.work.step, SandboxWorkStep::RunInstall);
        assert_eq!(stored.work.attempts, 0);
        let home = stored.lifecycle.home.unwrap();
        let claim = home.native_pool.as_ref().unwrap().claim.as_ref().unwrap();
        assert_eq!(claim.uid, "claim-uid");
        assert_eq!(claim.sandbox_name.as_deref(), Some("warm-member"));
        assert!(home.receipt.is_some());
        for operation in ["select", "claim", "adopt"] {
            assert_eq!(calls(&pools, operation), 1);
        }
        assert_eq!(calls(&pools, "cleanup"), 0);
        assert_serial(&pools.observation);
        elapsed_ms.push(started.elapsed().as_millis());
        counts.push(pools.observation.starts.lock().unwrap().len());
    }
    println!(
        "CLAIM_CADENCE_MEASUREMENT {}",
        serde_json::json!({
            "response_ms":100,"ready_ms":(501..=841).step_by(10).collect::<Vec<_>>(),
            "elapsed_ms":elapsed_ms,"read_counts":counts,"samples":elapsed_ms.len(),
        })
    );
}

#[tokio::test(start_paused = true)]
async fn claim_cadence_slow_reads_do_not_overlap_or_catch_up() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let pools = pools(&h, &[]);
    pools.pending_claim.store(true, Ordering::SeqCst);
    *pools.observation.delay.lock().unwrap() = Duration::from_millis(400);
    let worker = worker(&h, pools.clone());
    let started = Instant::now();
    let assign = async {
        tokio::time::sleep(Duration::from_millis(450)).await;
        pools.pending_claim.store(false, Ordering::SeqCst);
    };
    let (result, ()) = tokio::join!(worker.run_one(&created.id, start_time(&created)), assign);
    assert!(result.unwrap());
    assert!(h.stored(&created.id).await.runtime.is_some());
    assert_serial(&pools.observation);
    assert_eq!(started.elapsed(), Duration::from_millis(1200));
    let starts = pools.observation.starts.lock().unwrap();
    assert_eq!(
        starts.iter().map(|at| *at - started).collect::<Vec<_>>(),
        [
            Duration::ZERO,
            Duration::from_millis(400),
            Duration::from_millis(800)
        ]
    );
}

#[tokio::test(start_paused = true)]
async fn claim_cadence_pending_deadline_retains_identity_and_bounds_reads() {
    for delay_ms in [0, 100, 400] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        pools.pending_claim.store(true, Ordering::SeqCst);
        *pools.observation.delay.lock().unwrap() = Duration::from_millis(delay_ms);
        let worker = worker(&h, pools.clone());
        let started = Instant::now();
        let now = start_time(&created);
        assert!(worker.run_one(&created.id, now).await.unwrap());
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        let stored = h.stored(&created.id).await;
        let original = stored.lifecycle.home.clone().unwrap();
        assert_eq!(
            original
                .native_pool
                .as_ref()
                .unwrap()
                .claim
                .as_ref()
                .unwrap()
                .uid,
            "claim-uid"
        );
        assert!(original.receipt.is_none());
        assert_eq!(
            stored.work.ready_at,
            rfc3339_at(now + Duration::from_secs(30))
        );
        assert_serial(&pools.observation);
        let starts = pools.observation.starts.lock().unwrap().clone();
        assert!(starts.len() <= 20);
        assert!(starts
            .iter()
            .all(|at| *at < started + Duration::from_secs(5)));
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(*pools.observation.starts.lock().unwrap(), starts);
        assert_eq!(calls(&pools, "claim"), 1);
        assert_eq!(calls(&pools, "adopt"), 0);
        assert_eq!(
            h.stored(&created.id).await.lifecycle.home.as_deref(),
            Some(original.as_ref())
        );
    }
}

#[tokio::test(start_paused = true)]
async fn claim_cadence_counts_response_time_without_delaying_first_observation() {
    for pending in [false, true] {
        let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
        let created = h.create().await;
        let pools = pools(&h, &[]);
        pools.pending_claim.store(pending, Ordering::SeqCst);
        pools.observation.set_delay(Duration::from_millis(100));
        let worker = worker(&h, pools.clone());
        let started = Instant::now();
        let assign = async {
            if pending {
                tokio::time::sleep(Duration::from_millis(451)).await;
                pools.pending_claim.store(false, Ordering::SeqCst);
            }
        };
        let (result, ()) = tokio::join!(worker.run_one(&created.id, start_time(&created)), assign);
        assert!(result.unwrap());
        assert!(h.stored(&created.id).await.runtime.is_some());
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(if pending { 600 } else { 100 })
        );
        assert_serial(&pools.observation);
        assert_eq!(
            pools.observation.starts.lock().unwrap().len(),
            if pending { 3 } else { 1 }
        );
        assert_eq!(calls(&pools, "claim"), 1);
        assert_eq!(calls(&pools, "adopt"), 1);
    }
}

#[tokio::test(start_paused = true)]
async fn claim_cadence_cancels_an_inflight_read_without_detached_observation() {
    let h = Harness::new(vec![], 0, Duration::from_secs(60)).await;
    let created = h.create().await;
    let pools = pools(&h, &[]);
    pools.pending_claim.store(true, Ordering::SeqCst);
    pools.observation.set_delay(Duration::from_millis(400));
    let worker = worker(&h, pools.clone());
    let id = created.id.clone();
    let now = start_time(&created);
    let job = tokio::spawn(async move { worker.run_one(&id, now).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(pools.observation.active.load(Ordering::SeqCst), 1);
    let original = h.stored(&created.id).await.lifecycle.home;
    job.abort();
    assert!(job.await.unwrap_err().is_cancelled());
    assert_serial(&pools.observation);
    tokio::time::advance(Duration::from_secs(10)).await;
    assert_eq!(pools.observation.starts.lock().unwrap().len(), 1);
    assert_eq!(h.stored(&created.id).await.lifecycle.home, original);
    assert_eq!(calls(&pools, "claim"), 1);
    assert_eq!(calls(&pools, "adopt"), 0);
}
