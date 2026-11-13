use super::*;

#[tokio::test]
async fn successes_failures_and_cancellation_retain_only_consumed_lengths() {
    isolated_test(async {
        let raw = mongodb::bson::rawdoc! {"private":"fixture payload"};
        let bytes = raw.as_bytes().len() as u64;
        {
            let mut read = Read::find("agent_log");
            read.document(raw.as_bytes().len());
            read.complete();
        }
        {
            let mut read = Read::find("agent_log");
            read.complete();
        }
        drop(Read::find("agent_log")); // query failure before receiving a document
        let interrupted = async {
            let mut read = Read::find("agent_log");
            read.document(raw.as_bytes().len());
            std::future::pending::<()>().await;
            read.complete();
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(1), interrupted)
                .await
                .is_err()
        );
        assert_eq!(snapshot("agent_log"), [4, 2, 2, 2 * bytes]);
        assert_eq!(snapshot("agents"), [0; 4]);
    })
    .await;
}

#[test]
fn collection_labels_are_bounded_and_unknown_names_are_never_retained() {
    let classes: std::collections::HashSet<_> =
        COLLECTIONS.iter().map(|name| class(name)).collect();
    assert_eq!(classes.len(), COLLECTIONS.len());
    assert!(COLLECTIONS.len() <= 256);
    for index in 0..10_000 {
        assert_eq!(class(&format!("private_{index}")), COLLECTIONS.len());
    }
    let total = AtomicU64::new(u64::MAX);
    add(&total, 1);
    assert_eq!(total.load(Ordering::Relaxed), u64::MAX);
}

#[derive(Clone, Default)]
struct Output(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
#[test]
fn emitted_rows_are_numeric_content_free_and_have_one_operation_label() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let private = "private request and collection identity";
    let span = tracing::info_span!("request", secret = private);
    let _entered = span.enter();
    let counters = Arc::new(Counters::default());
    {
        let mut read = Read {
            counters: counters.clone(),
            class: class(private),
            documents: 0,
            bytes: 0,
            success: false,
        };
        read.document(123);
        read.complete();
    }
    assert!(output.0.lock().unwrap().is_empty());
    counters.emit();
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(!logs.contains(private));
    assert!(logs.contains("collection=\"other\"") && logs.contains("operation=\"find\""));
    assert!(logs.contains("cma_mongo_read_document_bytes_total") && logs.contains("value=123"));
    assert_eq!(logs.lines().count(), 4);
    counters.emit();
    assert_eq!(output.0.lock().unwrap().len(), logs.len());
}

#[tokio::test(start_paused = true)]
async fn final_sample_includes_completed_and_cancelled_worker_drain_reads() {
    use tracing::instrument::WithSubscriber;

    // Reproduce the review's late 137-byte read, then cover the total-deadline
    // cancellation path too. Each scope has independent counters and log output.
    for completes in [true, false] {
        isolated_test(async {
            let output = Output::default();
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_writer(output.clone())
                .finish();
            let mut shutdown = crate::shutdown::ShutdownCoordinator::new();
            let timer = crate::read_telemetry::spawn(shutdown.subscribe());
            let signal = shutdown.subscribe();
            let shared = counters();
            let (started, ready) = tokio::sync::oneshot::channel();
            shutdown.push(tokio::spawn(TEST_COUNTERS.scope(shared, async move {
                let mut read = Read::find("agents");
                read.document(137);
                started.send(()).unwrap();
                crate::shutdown::signalled(signal).await;
                // The periodic exporter exits before this worker releases its
                // guard, just as it does when an in-flight read is draining.
                timer.await.unwrap();
                if completes {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    read.complete();
                } else {
                    std::future::pending::<()>().await;
                }
            })));
            ready.await.unwrap();
            assert_eq!(snapshot("agents"), [0; 4]);
            let aborted = shutdown.drain().with_subscriber(subscriber).await;
            assert_eq!(aborted, usize::from(!completes));
            assert_eq!(snapshot("agents"), [1, u64::from(!completes), 1, 137]);
            let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
            for (metric, value) in [
                ("cma_mongo_read_operations_total", 1),
                ("cma_mongo_read_failures_total", u64::from(!completes)),
                ("cma_mongo_read_documents_total", 1),
                ("cma_mongo_read_document_bytes_total", 137),
            ] {
                let rows: Vec<_> = logs.lines().filter(|line| line.contains(metric)).collect();
                assert_eq!(rows.len(), 1, "{logs}");
                assert!(rows[0].contains("collection=\"agents\""));
                assert!(rows[0].ends_with(&format!("value={value}")), "{}", rows[0]);
            }
        })
        .await;
    }
}

#[tokio::test(start_paused = true)]
async fn scoped_document_counts_isolate_collections_nested_work_and_cancelled_reads() {
    let outer = DocumentObserver::new("agent_log");
    let nested = DocumentObserver::new("agent_log");
    outer
        .scope(async {
            Read::find("agents").document(10);
            Read::find("agent_log").document(10);
            nested
                .scope(async {
                    Read::find("agent_log").document(10);
                })
                .await;
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), async {
                    Read::find("agent_log").document(10);
                    std::future::pending::<()>().await;
                })
                .await
                .is_err()
            );
        })
        .await;
    Read::find("agent_log").document(10);
    assert_eq!(outer.documents(), 2);
    assert_eq!(nested.documents(), 1);
}
