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
