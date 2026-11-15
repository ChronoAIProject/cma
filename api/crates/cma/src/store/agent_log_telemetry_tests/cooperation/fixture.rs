use super::*;

#[derive(Clone)]
pub(super) struct StepClock {
    base: Instant,
    ticks: Arc<AtomicU64>,
}
impl StepClock {
    pub(super) fn new() -> Self {
        Self {
            base: Instant::now(),
            ticks: Arc::default(),
        }
    }
}
impl Clock for StepClock {
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.ticks.fetch_add(1, Ordering::Relaxed))
    }
}

pub(super) async fn client() -> Client {
    Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap()
}

pub(super) fn database(client: &Client) -> Database {
    client.database(&format!(
        "cma_scan_cooperation_{}",
        uuid::Uuid::new_v4().simple()
    ))
}

pub(super) fn entries(id: &AgentId, count: u64, content_bytes: usize) -> Vec<AgentLogEntry> {
    (1..=count)
        .map(|seq| AgentLogEntry {
            agent_id: id.clone(),
            seq,
            appended_at: "2026-11-13T00:00:00Z".into(),
            kind: AgentLogKind::AgentMessage {
                content: "x".repeat(content_bytes),
            },
        })
        .collect()
}

pub(super) async fn insert(collection: &Collection<Document>, entries: &[AgentLogEntry]) -> u64 {
    let documents: Vec<_> = entries
        .iter()
        .map(|entry| log_document(entry).unwrap())
        .collect();
    let bytes = documents
        .iter()
        .map(|doc| doc.to_vec().unwrap().len() as u64)
        .sum();
    collection.insert_many(documents).await.unwrap();
    bytes
}

pub(super) fn options(enabled: bool) -> ScanOptions<StepClock> {
    ScanOptions::new(false, Policy::enabled(enabled)).with_clock(StepClock::new())
}
