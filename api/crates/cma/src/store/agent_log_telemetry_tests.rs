use super::*;

#[tokio::test]
async fn every_fixed_caller_counts_success_failure_and_cancellation() {
    isolated_test(async {
        for caller in CALLERS {
            {
                let mut scan = Scan::new(caller);
                scan.document(127);
                scan.document(233);
                scan.started -= std::time::Duration::from_millis(1);
                scan.complete();
            }
            {
                let mut empty = Scan::new(caller);
                empty.complete();
            }
            drop(Scan::new(caller)); // pre-read refusal
            {
                let mut partial = Scan::new(caller);
                partial.document(59); // consumed offending BSON on validation failure
            }
            let cancelled = async {
                let mut scan = Scan::new(caller);
                scan.document(73);
                std::future::pending::<()>().await;
                scan.complete();
            };
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(1), cancelled)
                    .await
                    .is_err()
            );
            let success = snapshot(caller, true);
            assert_eq!(&success[..3], &[2, 2, 360], "{caller:?}");
            assert!(success[3] >= 1_000);
            let failed = snapshot(caller, false);
            assert_eq!(&failed[..3], &[3, 2, 132], "{caller:?}");
            assert!(failed[3] >= 1_000);
        }
    })
    .await;
}

#[test]
fn caller_and_step_inventory_is_closed_unique_and_exhaustive() {
    let names: std::collections::HashSet<_> = CALLERS.iter().map(|c| c.name()).collect();
    assert_eq!(
        names,
        std::collections::HashSet::from([
            "worker_publication",
            "capacity_settlement",
            "runtime_reconciliation",
            "recovery_proof",
            "inactivity_proof",
            "title_discovery",
            "public_window_prefix",
            "conversation_publication",
            "checkpoint",
            "command_admission",
            "unknown_commit_proof",
            "other",
        ])
    );
    let mut pairs = std::collections::HashSet::new();
    for (index, caller) in CALLERS.into_iter().enumerate() {
        assert_eq!(caller.index(), index);
        assert!(pairs.insert((caller.name(), caller.worker_step())));
        match caller {
            ScanCaller::WorkerPublication(step) | ScanCaller::CapacitySettlement(step) => {
                assert_eq!(caller.worker_step(), step.operation_name());
            }
            _ => assert_eq!(caller.worker_step(), "not_applicable"),
        }
    }
    let total = AtomicU64::new(u64::MAX - 1);
    add(&total, 9);
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
fn emission_is_batched_numeric_and_cannot_inherit_private_span_fields() {
    let output = Output::default();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(output.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let span = tracing::info_span!(
        "private_request",
        agent_id = "private_agent_identity",
        error = "raw driver error and user content"
    );
    let _entered = span.enter();
    let counters = Arc::new(Counters::default());
    for caller in CALLERS {
        let mut scan = Scan::with_counters(counters.clone(), caller);
        scan.document(123);
        scan.complete();
    }
    assert!(output.0.lock().unwrap().is_empty());
    counters.emit();
    let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert_eq!(logs.lines().count(), 4 * CALLERS.len());
    for row in logs.lines() {
        assert!(
            row.contains("caller=")
                && row.contains("worker_step=")
                && row.contains("outcome=\"success\"")
        );
        assert!(
            !row.contains("private")
                && !row.contains("raw driver")
                && !row.contains("user content")
        );
    }
    assert!(logs.contains("cma_agent_log_scan_bytes_total") && logs.contains("value=123"));
    assert!(logs.contains("cma_agent_log_scan_duration_microseconds_total"));
    counters.emit();
    assert_eq!(output.0.lock().unwrap().len(), logs.len());
}

#[tokio::test(start_paused = true)]
async fn final_emission_includes_scans_completed_or_cancelled_during_worker_drain() {
    use tracing::instrument::WithSubscriber;
    for completes in [false, true] {
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
                let mut scan = Scan::new(ScanCaller::Checkpoint);
                scan.document(137);
                started.send(()).unwrap();
                crate::shutdown::signalled(signal).await;
                timer.await.unwrap();
                if completes {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    scan.complete();
                } else {
                    std::future::pending::<()>().await;
                }
            })));
            ready.await.unwrap();
            let aborted = shutdown.drain().with_subscriber(subscriber).await;
            assert_eq!(aborted, usize::from(!completes));
            assert_eq!(
                &snapshot(ScanCaller::Checkpoint, completes)[..3],
                &[1, 1, 137]
            );
            let logs = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
            let scan_rows: Vec<_> = logs
                .lines()
                .filter(|line| line.contains("cma_agent_log_scan"))
                .collect();
            assert_eq!(scan_rows.len(), 4, "{logs}");
            assert!(scan_rows
                .iter()
                .all(|row| row.contains("caller=\"checkpoint\"")));
        })
        .await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn real_complete_scan_counts_success_corruption_and_failed_reads() {
    use super::super::{agent_log::log_document, agent_log_integrity::scan_collection};
    use layer2_core::{
        ids::AgentId,
        sandbox::{AgentLogEntry, AgentLogKind},
    };
    use mongodb::{
        bson::{doc, Document},
        Client,
    };
    let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database = client.database(&format!("cma_scan_{}", uuid::Uuid::new_v4().simple()));
    let collection = database.collection::<Document>("agent_log");
    let id = AgentId("agt_scan_fixture".into());
    let document = log_document(&AgentLogEntry {
        agent_id: id.clone(),
        seq: 1,
        appended_at: "2026-11-08T00:00:00Z".into(),
        kind: AgentLogKind::Deleted,
    })
    .unwrap();
    let bytes = document.to_vec().unwrap().len() as u64;
    collection.insert_one(document).await.unwrap();
    super::super::mongo_read_telemetry::isolated_test(isolated_test(async {
        let mut session = client.start_session().await.unwrap();
        for caller in CALLERS {
            assert!(scan_collection(
                caller,
                &collection,
                &id,
                1,
                &mut session,
                crate::store::agent_log_integrity::ScanOptions::new(false, Default::default()),
                |_| Ok(())
            )
            .await
            .is_ok());
            assert_eq!(&snapshot(caller, true)[..3], &[1, 1, bytes]);
            // Missing tail retains consumption without successful validation.
            assert!(scan_collection(
                caller,
                &collection,
                &id,
                2,
                &mut session,
                crate::store::agent_log_integrity::ScanOptions::new(false, Default::default()),
                |_| Ok(())
            )
            .await
            .is_err());
            assert_eq!(&snapshot(caller, false)[..3], &[1, 1, bytes]);
        }
        assert_eq!(
            super::super::mongo_read_telemetry::snapshot("agent_log"),
            [
                2 * CALLERS.len() as u64,
                CALLERS.len() as u64,
                2 * CALLERS.len() as u64,
                2 * CALLERS.len() as u64 * bytes
            ]
        );
        let caller = ScanCaller::Other;
        collection
            .update_one(doc! {}, doc! {"$set": {"seq": 7_i64}})
            .await
            .unwrap();
        assert!(scan_collection(
            caller,
            &collection,
            &id,
            1,
            &mut session,
            crate::store::agent_log_integrity::ScanOptions::new(false, Default::default()),
            |_| Ok(())
        )
        .await
        .is_err());
        assert_eq!(&snapshot(caller, false)[..3], &[2, 2, 2 * bytes]);
        let invalid = database.collection::<Document>("invalid\0collection");
        assert!(scan_collection(
            caller,
            &invalid,
            &id,
            1,
            &mut session,
            crate::store::agent_log_integrity::ScanOptions::new(false, Default::default()),
            |_| Ok(())
        )
        .await
        .is_err());
        assert_eq!(&snapshot(caller, false)[..3], &[3, 2, 2 * bytes]);
        assert_eq!(
            super::super::mongo_read_telemetry::snapshot("other"),
            [1, 1, 0, 0]
        );
        // Physical head refusal happens before querying and counts zero consumed bytes.
        assert!(scan_collection(
            caller,
            &collection,
            &id,
            10_129,
            &mut session,
            crate::store::agent_log_integrity::ScanOptions::new(false, Default::default()),
            |_| Ok(())
        )
        .await
        .is_err());
        assert_eq!(&snapshot(caller, false)[..3], &[4, 2, 2 * bytes]);
    }))
    .await;
    database.drop().await.unwrap();
}

#[test]
fn scan_execution_export_is_once_per_attempt_including_failure() {
    for complete in [false, true] {
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::with_local_recorder(&recorder, || {
            let mut scan = Scan::new(ScanCaller::Other);
            for _ in 0..100 {
                scan.document(128);
            }
            if complete {
                scan.complete();
            }
            drop(scan);
        });
        let text = handle.render();
        for metric in [
            "cma_agent_log_scan_longest_section_seconds",
            "cma_agent_log_scan_driver_poll_seconds",
        ] {
            assert!(
                text.contains(&format!(r#"{metric}_count{{caller="other"}} 1"#)),
                "{text}"
            );
        }
        assert!(text.contains(r#"cma_agent_log_scan_yields_total{caller="other"} 0"#));
        for private in ["agent_id", "sandbox_id", "request_id", "user_id"] {
            assert!(!text.contains(private));
        }
    }
}

#[path = "agent_log_telemetry_tests/cooperation.rs"]
mod cooperation;
