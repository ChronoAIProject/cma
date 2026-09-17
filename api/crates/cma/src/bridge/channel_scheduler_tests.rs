use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

struct Port {
    seen: Mutex<Vec<String>>,
    active: AtomicUsize,
    peak: AtomicUsize,
}
#[async_trait::async_trait]
impl ChannelWorkPort for Port {
    fn kind(&self) -> crate::bridge::operations::WorkKind {
        crate::bridge::operations::WorkKind::Fixture
    }
    async fn discover(&self, after: Option<&str>) -> Result<Vec<String>, ChannelWorkError> {
        Ok((0..70)
            .map(|n| format!("row{n:03}"))
            .filter(|id| after.is_none_or(|after| id.as_str() > after))
            .take(32)
            .collect())
    }
    async fn run_one(&self, id: &str) -> Result<(), ChannelWorkError> {
        let current = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(2)).await;
        self.seen.lock().unwrap().push(id.to_owned());
        self.active.fetch_sub(1, Ordering::SeqCst);
        if id == "row000" {
            Err(ChannelWorkError::Corrupt)
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn failed_first_record_does_not_starve_later_pages_and_concurrency_is_bounded() {
    let port = Arc::new(Port {
        seen: Mutex::new(Vec::new()),
        active: AtomicUsize::new(0),
        peak: AtomicUsize::new(0),
    });
    let (stop, signal) = watch::channel(false);
    let task = ChannelScheduler::new(port.clone()).spawn(signal);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if port.seen.lock().unwrap().iter().any(|id| id == "row069") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    task.await.unwrap();
    assert!(port.peak.load(Ordering::SeqCst) <= 4);
    assert_eq!(port.active.load(Ordering::SeqCst), 0);
    let seen = port.seen.lock().unwrap();
    assert!(seen.iter().any(|id| id == "row000"));
    assert!(seen.iter().any(|id| id == "row032"));
}

struct InvalidPage;
#[async_trait::async_trait]
impl ChannelWorkPort for InvalidPage {
    fn kind(&self) -> crate::bridge::operations::WorkKind {
        crate::bridge::operations::WorkKind::Fixture
    }
    async fn discover(&self, _: Option<&str>) -> Result<Vec<String>, ChannelWorkError> {
        Ok(vec!["repeated".into(), "repeated".into()])
    }
    async fn run_one(&self, _: &str) -> Result<(), ChannelWorkError> {
        panic!("invalid discovery must never execute");
    }
}

#[tokio::test]
async fn invalid_discovery_cannot_publish_candidates_or_advance_cursor() {
    let mut scheduled = Scheduled::new(Arc::new(InvalidPage));
    scheduled.poll();
    let completed = scheduled.tasks.join_next_with_id().await.unwrap();
    scheduled.complete(completed);
    scheduled.fill();
    assert!(scheduled.cursor.is_none());
    assert!(scheduled.queued.is_empty());
    assert!(scheduled.tasks.is_empty());
}

struct RetryPort {
    seen: Mutex<Vec<String>>,
}

struct TransientPort {
    seen: Mutex<Vec<String>>,
    failure: ChannelWorkError,
}
#[async_trait::async_trait]
impl ChannelWorkPort for TransientPort {
    fn kind(&self) -> crate::bridge::operations::WorkKind {
        crate::bridge::operations::WorkKind::Inbox
    }
    async fn discover(&self, _: Option<&str>) -> Result<Vec<String>, ChannelWorkError> {
        Ok(vec!["transient".into(), "unrelated".into()])
    }
    async fn run_one(&self, id: &str) -> Result<(), ChannelWorkError> {
        let mut seen = self.seen.lock().unwrap();
        seen.push(id.into());
        if id == "transient" && seen.iter().filter(|id| id.as_str() == "transient").count() == 1 {
            Err(self.failure)
        } else {
            Ok(())
        }
    }
}

#[tokio::test(start_paused = true)]
async fn transient_input_recovers_promptly_without_hot_retry_or_blocking_another_message() {
    for (failure, delay) in [
        (ChannelWorkError::Unavailable, Duration::from_secs(1)),
        (ChannelWorkError::Changed, Duration::from_millis(250)),
    ] {
        let port = Arc::new(TransientPort {
            seen: Mutex::new(Vec::new()),
            failure,
        });
        let mut scheduled = Scheduled::new(port.clone());
        discover_and_drain(&mut scheduled).await;
        tokio::time::advance(delay - Duration::from_millis(1)).await;
        discover_and_drain(&mut scheduled).await;
        {
            let seen = port.seen.lock().unwrap();
            assert_eq!(
                seen.iter().filter(|id| id.as_str() == "transient").count(),
                1
            );
            assert_eq!(
                seen.iter().filter(|id| id.as_str() == "unrelated").count(),
                2
            );
        }
        tokio::time::advance(Duration::from_millis(1)).await;
        discover_and_drain(&mut scheduled).await;
        assert_eq!(
            port.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|id| id.as_str() == "transient")
                .count(),
            2
        );
        assert!(!scheduled.retry_after.contains_key("transient"));
    }
}

#[async_trait::async_trait]
impl ChannelWorkPort for RetryPort {
    fn kind(&self) -> crate::bridge::operations::WorkKind {
        crate::bridge::operations::WorkKind::Fixture
    }
    async fn discover(&self, _: Option<&str>) -> Result<Vec<String>, ChannelWorkError> {
        Ok(vec!["allocation".into(), "unrelated".into()])
    }
    async fn run_one(&self, id: &str) -> Result<(), ChannelWorkError> {
        self.seen.lock().unwrap().push(id.to_owned());
        if id == "allocation" {
            Err(ChannelWorkError::Suspended)
        } else {
            Ok(())
        }
    }
}

async fn discover_and_drain(scheduled: &mut Scheduled) {
    scheduled.poll();
    scheduled.fill();
    while let Some(completed) = scheduled.tasks.join_next_with_id().await {
        scheduled.complete(completed);
        scheduled.fill();
    }
}

#[tokio::test(start_paused = true)]
async fn suspended_allocation_waits_before_retry_without_delaying_unrelated_work() {
    let port = Arc::new(RetryPort {
        seen: Mutex::new(Vec::new()),
    });
    let mut scheduled = Scheduled::new(port.clone());
    discover_and_drain(&mut scheduled).await;
    for _ in 0..119 {
        tokio::time::advance(Duration::from_millis(250)).await;
        discover_and_drain(&mut scheduled).await;
    }
    {
        let seen = port.seen.lock().unwrap();
        assert_eq!(
            seen.iter().filter(|id| id.as_str() == "allocation").count(),
            1
        );
        assert_eq!(
            seen.iter().filter(|id| id.as_str() == "unrelated").count(),
            120
        );
    }
    tokio::time::advance(Duration::from_millis(250)).await;
    discover_and_drain(&mut scheduled).await;
    assert_eq!(
        port.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == "allocation")
            .count(),
        2
    );
}

#[tokio::test(start_paused = true)]
async fn invalid_discovery_waits_before_contacting_the_store_again() {
    let mut scheduled = Scheduled::new(Arc::new(InvalidPage));
    discover_and_drain(&mut scheduled).await;
    tokio::time::advance(Duration::from_secs(29)).await;
    scheduled.poll();
    assert!(scheduled.tasks.is_empty());
    tokio::time::advance(Duration::from_secs(1)).await;
    scheduled.poll();
    assert_eq!(scheduled.tasks.len(), 1);
    let completed = scheduled.tasks.join_next_with_id().await.unwrap();
    scheduled.complete(completed);
}

#[tokio::test(start_paused = true)]
async fn retry_window_is_bounded_and_resumes_after_capacity_pause() {
    let port = Arc::new(RetryPort {
        seen: Mutex::new(Vec::new()),
    });
    let mut scheduled = Scheduled::new(port.clone());
    let deadline = tokio::time::Instant::now() + FAILURE_RETRY_DELAY;
    for n in 0..MAX_RETRY_ENTRIES {
        scheduled.retry_after.insert(format!("prior{n}"), deadline);
    }
    discover_and_drain(&mut scheduled).await;
    assert_eq!(scheduled.retry_after.len(), MAX_RETRY_ENTRIES);
    tokio::time::advance(Duration::from_secs(29)).await;
    scheduled.poll();
    scheduled.fill();
    assert!(scheduled.tasks.is_empty());
    tokio::time::advance(Duration::from_secs(1)).await;
    discover_and_drain(&mut scheduled).await;
    assert!(scheduled.retry_after.len() < MAX_RETRY_ENTRIES);
    assert_eq!(
        port.seen
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == "allocation")
            .count(),
        2
    );
}
