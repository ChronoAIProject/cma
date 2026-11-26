use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Store {
    calls: AtomicUsize,
    unavailable: AtomicBool,
}
#[async_trait::async_trait]
impl SharedControlRate for Store {
    async fn charge(&self, _: &str, _: u32, burst: u32) -> Result<Option<u64>, ()> {
        assert_eq!(burst, 1);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.unavailable.load(Ordering::SeqCst) {
            Err(())
        } else {
            Ok(None)
        }
    }
}
struct Clock(Mutex<Instant>);
impl AdmissionClock for Clock {
    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }
}

fn fixture(enabled: bool, max_callers: usize) -> (Policy, Arc<Store>, Arc<Clock>) {
    let store = Arc::new(Store {
        calls: AtomicUsize::new(0),
        unavailable: AtomicBool::new(false),
    });
    let clock = Arc::new(Clock(Mutex::new(Instant::now())));
    (
        Policy::new(
            ControlAdmissionConfig {
                enabled,
                max_callers,
                ..Default::default()
            },
            store.clone(),
            clock.clone(),
        ),
        store,
        clock,
    )
}

#[tokio::test]
async fn disabled_policy_never_reads_or_writes_quota() {
    let (policy, store, _) = fixture(false, 1);
    for _ in 0..100 {
        assert!(policy
            .admit("caller", "agent", Class::Stop)
            .await
            .unwrap()
            .is_none());
    }
    assert_eq!(store.calls.load(Ordering::SeqCst), 0);
    assert!(policy.cooldowns.lock().unwrap().is_empty());
}

#[tokio::test]
async fn caller_class_isolation_and_local_storm_suppression() {
    let (policy, store, clock) = fixture(true, 4);
    drop(
        policy
            .admit("caller", "agent", Class::Delete)
            .await
            .unwrap(),
    );
    for _ in 0..100 {
        assert!(matches!(
            policy.admit("caller", "agent", Class::Delete).await,
            Err(Refusal::Caller(_))
        ));
    }
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    drop(policy.admit("caller", "agent", Class::Stop).await.unwrap());
    drop(policy.admit("other", "agent", Class::Stop).await.unwrap());
    assert_eq!(store.calls.load(Ordering::SeqCst), 3);
    *clock.0.lock().unwrap() += Duration::from_secs(10);
    drop(policy.admit("caller", "agent", Class::Stop).await.unwrap());
    assert!(matches!(
        policy.admit("caller", "agent", Class::Delete).await,
        Err(Refusal::Caller(_))
    ));
    *clock.0.lock().unwrap() += Duration::from_secs(10);
    assert!(policy.admit("caller", "agent", Class::Delete).await.is_ok());
}

#[tokio::test]
async fn bounded_map_does_not_reset_allowance_on_eviction() {
    let (policy, store, clock) = fixture(true, 1);
    drop(policy.admit("caller", "agent", Class::Stop).await.unwrap());
    assert!(matches!(
        policy.admit("other", "other", Class::Stop).await,
        Err(Refusal::Capacity)
    ));
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    *clock.0.lock().unwrap() += Duration::from_secs(10);
    assert!(policy.admit("other", "other", Class::Stop).await.is_ok());
    assert_eq!(policy.cooldowns.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn quota_failure_refuses_and_releases_local_capacity() {
    let (policy, store, clock) = fixture(true, 4);
    store.unavailable.store(true, Ordering::SeqCst);
    assert!(matches!(
        policy.admit("caller", "agent", Class::Stop).await,
        Err(Refusal::Unavailable)
    ));
    assert_eq!(store.calls.load(Ordering::SeqCst), 1);
    store.unavailable.store(false, Ordering::SeqCst);
    *clock.0.lock().unwrap() += Duration::from_secs(1);
    assert!(policy.admit("caller", "agent", Class::Stop).await.is_ok());
}

struct DelayedStore {
    started: tokio::sync::Notify,
    finish: tokio::sync::Semaphore,
}

#[async_trait::async_trait]
impl SharedControlRate for DelayedStore {
    async fn charge(&self, _: &str, _: u32, _: u32) -> Result<Option<u64>, ()> {
        self.started.notify_one();
        self.finish.acquire().await.unwrap().forget();
        Ok(None)
    }
}

#[tokio::test(start_paused = true)]
async fn timed_out_or_cancelled_quota_observer_retains_capacity_until_io_finishes() {
    for cancel in [false, true] {
        let store = Arc::new(DelayedStore {
            started: tokio::sync::Notify::new(),
            finish: tokio::sync::Semaphore::new(0),
        });
        let policy = Arc::new(Policy::new(
            ControlAdmissionConfig {
                enabled: true,
                ..Default::default()
            },
            store.clone(),
            Arc::new(SystemClock),
        ));
        let running = policy.clone();
        let observer =
            tokio::spawn(async move { running.admit("caller", "agent", Class::Stop).await });
        store.started.notified().await;
        if cancel {
            observer.abort();
            assert!(matches!(observer.await, Err(error) if error.is_cancelled()));
        } else {
            tokio::time::advance(Duration::from_secs(2)).await;
            assert!(matches!(observer.await.unwrap(), Err(Refusal::Unavailable)));
        }
        assert!(matches!(
            policy
                .resources()
                .try_stage("caller", "agent", Class::Stop, 1),
            Err(Refusal::Caller(_))
        ));
        // Delete remains available to this caller while its Stop I/O is held.
        let delete = policy
            .resources()
            .try_stage("caller", "agent", Class::Delete, 1)
            .unwrap();
        let sentinel = policy
            .resources()
            .try_stage("other", "other", Class::Stop, 1)
            .unwrap();
        assert!(matches!(
            policy
                .resources()
                .try_stage("third", "third", Class::Stop, 1),
            Err(Refusal::Capacity)
        ));
        store.finish.add_permits(1);
        // The detached task must join and drop exactly its own permit.
        tokio::task::yield_now().await;
        let recovered = policy
            .resources()
            .try_stage("caller", "agent", Class::Stop, 1)
            .unwrap();
        drop((recovered, delete, sentinel));
        assert!(policy
            .resources()
            .try_stage("other", "other", Class::Stop, 1)
            .is_ok());
    }
}

struct RefusingStore(u64);
#[async_trait::async_trait]
impl SharedControlRate for RefusingStore {
    async fn charge(&self, _: &str, _: u32, _: u32) -> Result<Option<u64>, ()> {
        Ok(Some(self.0))
    }
}

#[tokio::test]
async fn shared_refusal_has_bounded_retry_after_and_releases_the_stage() {
    for (delay, expected) in [(0, 1), (20, 20), (u64::MAX, 60)] {
        let policy = Policy::new(
            ControlAdmissionConfig {
                enabled: true,
                ..Default::default()
            },
            Arc::new(RefusingStore(delay)),
            Arc::new(SystemClock),
        );
        assert!(matches!(
            policy.admit("caller", "agent", Class::Delete).await,
            Err(Refusal::Caller(actual)) if actual == expected
        ));
        assert!(policy
            .resources()
            .try_stage("caller", "agent", Class::Delete, 1)
            .is_ok());
    }
}
