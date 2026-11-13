use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use tokio::sync::Semaphore;

use super::*;

struct Work {
    pages: Mutex<VecDeque<Page<AgentId>>>,
    cursors: Mutex<Vec<Option<String>>>,
    calls: Mutex<Vec<AgentId>>,
    gates: Mutex<HashMap<AgentId, Arc<Semaphore>>>,
    active: AtomicUsize,
    maximum: AtomicUsize,
    query_gate: Option<Arc<Semaphore>>,
}

struct Active(Arc<Work>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl TitleScheduleWork for Work {
    async fn candidates(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentId>, AgentTitleServiceError> {
        assert_eq!(page.limit, PAGE_SIZE);
        self.cursors.lock().unwrap().push(page.cursor.clone());
        if let Some(gate) = &self.query_gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(self.pages.lock().unwrap().pop_front().unwrap_or(Page {
            items: vec![],
            next: None,
        }))
    }

    async fn run_one(
        self: Arc<Self>,
        id: &AgentId,
    ) -> Result<AgentTitleOutcome, AgentTitleServiceError> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum.fetch_max(active, Ordering::SeqCst);
        let _active = Active(self.clone());
        self.calls.lock().unwrap().push(id.clone());
        let gate = self.gates.lock().unwrap().get(id).cloned();
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(AgentTitleOutcome::Generated)
    }
}

fn work(pages: Vec<Page<AgentId>>) -> Arc<Work> {
    Arc::new(Work {
        pages: Mutex::new(pages.into()),
        cursors: Mutex::new(vec![]),
        calls: Mutex::new(vec![]),
        gates: Mutex::new(HashMap::new()),
        active: AtomicUsize::new(0),
        maximum: AtomicUsize::new(0),
        query_gate: None,
    })
}

fn page(ids: &[&str], next: Option<&str>) -> Page<AgentId> {
    Page {
        items: ids.iter().map(|id| AgentId((*id).into())).collect(),
        next: next.map(str::to_owned),
    }
}

fn spawn(worker: Arc<Work>) -> (watch::Sender<bool>, JoinHandle<()>) {
    let (shutdown, signal) = watch::channel(false);
    let task = AgentTitleScheduler {
        worker,
        concurrency: 2,
        poll_interval: Duration::from_millis(10),
        drain_timeout: Duration::from_millis(50),
        start_interval: Duration::ZERO,
    }
    .spawn(signal);
    (shutdown, task)
}

async fn until(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn empty_eligible_pages_keep_the_raw_cursor_and_finish_the_scan() {
    let worker = work(vec![
        page(&[], Some("raw-a")),
        page(&["b"], Some("raw-b")),
        page(&[], None),
    ]);
    let (shutdown, task) = spawn(worker.clone());
    until(|| worker.cursors.lock().unwrap().len() >= 4).await;
    shutdown.send(true).unwrap();
    task.await.unwrap();
    assert_eq!(
        &worker.cursors.lock().unwrap()[..4],
        &[None, Some("raw-a".into()), Some("raw-b".into()), None]
    );
    assert_eq!(*worker.calls.lock().unwrap(), vec![AgentId("b".into())]);
}

#[tokio::test]
async fn blocked_title_does_not_hold_other_titles_or_exceed_the_bound() {
    let worker = work(vec![page(&["a", "a", "b", "c", "d"], None)]);
    let gate = Arc::new(Semaphore::new(0));
    worker
        .gates
        .lock()
        .unwrap()
        .insert(AgentId("a".into()), gate.clone());
    let (shutdown, task) = spawn(worker.clone());
    until(|| worker.calls.lock().unwrap().len() == 4).await;
    assert_eq!(worker.maximum.load(Ordering::SeqCst), 2);
    assert_eq!(
        worker
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.as_str() == "a")
            .count(),
        1
    );
    gate.add_permits(1);
    shutdown.send(true).unwrap();
    task.await.unwrap();
    assert_eq!(worker.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn slow_discovery_is_single_flight_and_shutdown_cancels_owned_tasks() {
    let worker = Arc::new(Work {
        query_gate: Some(Arc::new(Semaphore::new(0))),
        ..Arc::try_unwrap(work(vec![])).ok().unwrap()
    });
    let (shutdown, task) = spawn(worker.clone());
    until(|| !worker.cursors.lock().unwrap().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(35)).await;
    assert_eq!(worker.cursors.lock().unwrap().len(), 1);
    shutdown.send(true).unwrap();
    task.await.unwrap();
    assert!(worker.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn shutdown_drains_only_started_titles_and_cancels_blocked_children() {
    let worker = work(vec![page(&["a", "b", "queued"], None)]);
    for id in ["a", "b"] {
        worker
            .gates
            .lock()
            .unwrap()
            .insert(AgentId(id.into()), Arc::new(Semaphore::new(0)));
    }
    let (shutdown, task) = spawn(worker.clone());
    until(|| worker.active.load(Ordering::SeqCst) == 2).await;
    shutdown.send(true).unwrap();
    task.await.unwrap();
    assert_eq!(worker.active.load(Ordering::SeqCst), 0);
    assert_eq!(worker.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn aborting_the_outer_scheduler_drops_its_running_children() {
    let worker = work(vec![page(&["a"], None)]);
    worker
        .gates
        .lock()
        .unwrap()
        .insert(AgentId("a".into()), Arc::new(Semaphore::new(0)));
    let (_shutdown, task) = spawn(worker.clone());
    until(|| worker.active.load(Ordering::SeqCst) == 1).await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    until(|| worker.active.load(Ordering::SeqCst) == 0).await;
}

#[tokio::test(start_paused = true)]
async fn historical_backfill_admits_at_most_one_title_per_ten_seconds() {
    let worker = work(vec![page(&["one", "two", "three"], None)]);
    let (shutdown, signal) = watch::channel(false);
    let scheduler = AgentTitleScheduler {
        worker: worker.clone(),
        concurrency: 4,
        poll_interval: Duration::from_secs(1),
        drain_timeout: Duration::from_secs(1),
        start_interval: START_INTERVAL,
    }
    .spawn(signal);
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(worker.calls.lock().unwrap().len(), 1);
    tokio::time::advance(Duration::from_secs(9)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(worker.calls.lock().unwrap().len(), 1);
    tokio::time::advance(Duration::from_secs(1)).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert_eq!(worker.calls.lock().unwrap().len(), 2);
    shutdown.send(true).unwrap();
    scheduler.await.unwrap();
}
