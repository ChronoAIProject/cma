use super::*;
use std::{
    future::{poll_fn, Future},
    sync::atomic::AtomicUsize,
    task::Poll,
};

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
    client.database(&format!("cma_owned_scan_{}", uuid::Uuid::new_v4().simple()))
}
pub(super) fn entries(id: &AgentId, count: u64, bytes: usize) -> Vec<AgentLogEntry> {
    (1..=count)
        .map(|seq| AgentLogEntry {
            agent_id: id.clone(),
            seq,
            appended_at: "2026-11-13T00:00:00Z".into(),
            kind: AgentLogKind::AgentMessage {
                content: "x".repeat(bytes),
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

#[derive(Clone, Default)]
pub(super) struct Observation(Arc<[AtomicU64; 4]>);
impl Observation {
    fn index(command: &str) -> Option<usize> {
        ["find", "getMore", "abortTransaction", "commitTransaction"]
            .iter()
            .position(|name| *name == command)
    }
    pub(super) async fn client(&self) -> Client {
        let mut options = mongodb::options::ClientOptions::parse(
            std::env::var("CHRONO_TEST_MONGODB_URI").unwrap(),
        )
        .await
        .unwrap();
        options.command_event_handler = None;
        options.tracing = Some(
            mongodb::otel::OpentelemetryOptions::builder()
                .enabled(true)
                .query_text_max_length(0usize)
                .tracer_provider(self.clone())
                .build(),
        );
        Client::with_options(options).unwrap()
    }
    pub(super) fn begin(&self) {
        for count in self.0.iter() {
            count.store(0, Ordering::Relaxed);
        }
    }
    pub(super) fn commands(&self, name: &str) -> u64 {
        Self::index(name)
            .map(|index| self.0[index].load(Ordering::Relaxed))
            .unwrap_or(0)
    }
}
impl opentelemetry::trace::TracerProvider for Observation {
    type Tracer = Self;
    fn tracer_with_scope(&self, _: opentelemetry::InstrumentationScope) -> Self {
        self.clone()
    }
}
impl opentelemetry::trace::Tracer for Observation {
    type Span = DiscardSpan;
    fn build_with_context(
        &self,
        builder: opentelemetry::trace::SpanBuilder,
        _: &opentelemetry::Context,
    ) -> DiscardSpan {
        let index = builder
            .attributes
            .as_deref()
            .unwrap_or_default()
            .iter()
            .find_map(
                |attribute| match (attribute.key.as_str(), &attribute.value) {
                    ("db.command.name", opentelemetry::Value::String(value)) => {
                        Self::index(value.as_str())
                    }
                    _ => None,
                },
            );
        if let Some(index) = index {
            self.0[index].fetch_add(1, Ordering::Relaxed);
        }
        DiscardSpan(opentelemetry::trace::SpanContext::NONE)
    }
}
pub(super) struct DiscardSpan(opentelemetry::trace::SpanContext);
impl opentelemetry::trace::Span for DiscardSpan {
    fn add_event_with_timestamp<T: Into<std::borrow::Cow<'static, str>>>(
        &mut self,
        _: T,
        _: std::time::SystemTime,
        _: Vec<opentelemetry::KeyValue>,
    ) {
    }
    fn span_context(&self) -> &opentelemetry::trace::SpanContext {
        &self.0
    }
    fn is_recording(&self) -> bool {
        false
    }
    fn set_attribute(&mut self, _: opentelemetry::KeyValue) {}
    fn set_status(&mut self, _: opentelemetry::trace::Status) {}
    fn update_name<T: Into<std::borrow::Cow<'static, str>>>(&mut self, _: T) {}
    fn add_link(&mut self, _: opentelemetry::trace::SpanContext, _: Vec<opentelemetry::KeyValue>) {}
    fn end_with_timestamp(&mut self, _: std::time::SystemTime) {}
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn admitted_cursor_keeps_snapshot_and_transaction_views_across_get_more() {
    let client = client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let writes = db.collection::<Document>("writes");
    writes.insert_one(doc! {"_id":"committed"}).await.unwrap();
    let id = AgentId("agt_owned_scan_snapshot".into());
    let expected = entries(&id, 300, 128);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for snapshot in [false, true] {
        for batches in [false, true] {
            collection.delete_many(doc! {}).await.unwrap();
            insert(&collection, &expected).await;
            let mut session = client.start_session().snapshot(snapshot).await.unwrap();
            if !snapshot {
                session
                    .start_transaction()
                    .read_concern(ReadConcern::snapshot())
                    .await
                    .unwrap();
                writes
                    .insert_one(doc! {"_id":"pending"})
                    .session(&mut session)
                    .await
                    .unwrap();
            }
            let visits = AtomicUsize::new(0);
            let mut retained = None;
            let mut work = Box::pin(
                scan(
                    &collection,
                    &id,
                    300,
                    &mut session,
                    ScanOptions::new(snapshot, Policy::enabled(true))
                        .with_clock(StepClock::new())
                        .with_raw_batches(batches),
                    &policy,
                    |value| {
                        let offset = visits.fetch_add(1, Ordering::Relaxed);
                        assert_eq!(*value, expected[offset]);
                        if offset == 149 {
                            retained = Some(value);
                        }
                        Ok(())
                    },
                )
                .unwrap(),
            );
            poll_fn(|cx| {
                assert!(work.as_mut().poll(cx).is_pending());
                if visits.load(Ordering::Relaxed) >= 8 {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            assert_eq!(visits.load(Ordering::Relaxed), 8);
            let mut changed = expected[149].clone();
            changed.kind = AgentLogKind::AgentMessage {
                content: "changed".repeat(65536),
            };
            collection
                .replace_one(doc! {"seq":150_i64}, log_document(&changed).unwrap())
                .await
                .unwrap();
            let mut appended = expected[299].clone();
            appended.seq = 301;
            insert(&collection, &[appended]).await;
            let summary = work.await.unwrap().transpose().unwrap();
            assert_eq!(summary.entry_count, 300);
            assert_eq!(visits.load(Ordering::Relaxed), 300);
            assert_eq!(**retained.as_ref().unwrap(), expected[149]);
            drop(summary);
            drop(retained);
            assert_eq!(root.available_bytes(), idle);
            if !snapshot {
                marker(&writes, &mut session).await;
                session.abort_transaction().await.unwrap();
            }
        }
    }
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn cancellation_and_mid_scan_pressure_discard_partial_state_and_keep_the_session() {
    let observation = Observation::default();
    let client = observation.client().await;
    let db = database(&client);
    let collection = db.collection::<Document>("agent_log");
    let writes = db.collection::<Document>("writes");
    writes.insert_one(doc! {"_id":"committed"}).await.unwrap();
    let id = AgentId("agt_owned_scan_cancel".into());
    let expected = entries(&id, 400, 64 * 1024);
    insert(&collection, &expected).await;
    let (root, policy) = policy();
    let idle = root.available_bytes();
    for batches in [false, true] {
        for cancel_after in [8, 112] {
            let mut session = client.start_session().await.unwrap();
            session.start_transaction().await.unwrap();
            writes
                .insert_one(doc! {"_id":"pending"})
                .session(&mut session)
                .await
                .unwrap();
            let visits = AtomicUsize::new(0);
            let clock = StepClock::new();
            let mut retained = None;
            observation.begin();
            isolated_test(async {
                let mut work = Box::pin(
                    scan(
                        &collection,
                        &id,
                        400,
                        &mut session,
                        ScanOptions::new(false, Policy::enabled(true))
                            .with_clock(clock.clone())
                            .with_raw_batches(batches),
                        &policy,
                        |value| {
                            let _ = clock.now();
                            visits.fetch_add(1, Ordering::Relaxed);
                            retained = Some(value);
                            Ok(())
                        },
                    )
                    .unwrap(),
                );
                poll_fn(|cx| {
                    assert!(work.as_mut().poll(cx).is_pending());
                    if visits.load(Ordering::Relaxed) >= cancel_after {
                        Poll::Ready(())
                    } else {
                        Poll::Pending
                    }
                })
                .await;
                assert_eq!(visits.load(Ordering::Relaxed), cancel_after);
                drop(work);
                assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
                assert_eq!(snapshot(ScanCaller::Other, false)[1], cancel_after as u64);
            })
            .await;
            assert_eq!(observation.commands("getMore") > 0, cancel_after > 101);
            assert_eq!(observation.commands("abortTransaction"), 0);
            assert_eq!(observation.commands("commitTransaction"), 0);
            assert_eq!(
                idle - root.available_bytes(),
                retained.as_ref().unwrap().requested_bytes()
            );
            drop(retained);
            assert_eq!(root.available_bytes(), idle);
            marker(&writes, &mut session).await;
            session.abort_transaction().await.unwrap();
        }
        let mut session = client.start_session().await.unwrap();
        session.start_transaction().await.unwrap();
        writes
            .insert_one(doc! {"_id":"pending"})
            .session(&mut session)
            .await
            .unwrap();
        let mut visits = 0;
        let mut pressure = None;
        let mut retained = None;
        let result = isolated_test(async {
            let result = scan(
                &collection,
                &id,
                400,
                &mut session,
                options(true).with_raw_batches(batches),
                &policy,
                |value| {
                    visits += 1;
                    if visits == 8 {
                        retained = Some(value);
                        pressure = Some(
                            policy
                                .admit(
                                    IngressClass::Data,
                                    AllocationPlan::checked(
                                        "mid-scan pressure",
                                        &[policy.budget(IngressClass::Data).available_bytes()],
                                    )
                                    .unwrap(),
                                )
                                .unwrap(),
                        );
                    }
                    Ok(())
                },
            )
            .unwrap()
            .await;
            assert_eq!(snapshot(ScanCaller::Other, true)[0], 0);
            assert_eq!(snapshot(ScanCaller::Other, false)[1], 9);
            result
        })
        .await;
        assert!(matches!(result, Err(AdmittedError::Refused(_))));
        assert_eq!(visits, 8);
        drop(pressure);
        drop(retained);
        assert_eq!(root.available_bytes(), idle);
        marker(&writes, &mut session).await;
        session.abort_transaction().await.unwrap();
    }
    db.drop().await.unwrap();
}
