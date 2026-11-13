//! Exact capacity markers use the same bounded proof as ordinary repeated retries.
use super::*;
use layer2_core::sandbox::store::plan_agent_cleanup_capacity_retry;

mod refusals;

const THIRD: &str = "2026-09-05T00:03:00Z";
const FOURTH: &str = "2026-09-05T00:04:00Z";

async fn seeded(step: AgentWorkStep) -> Fixture {
    let mut f = Fixture::new(100).await;
    position(&mut f, step).await;
    // Keep the observed 10,001-row shape without duplicating 40 MiB of test
    // messages. Historical failures are legitimate retained, immutable rows.
    let kind = retry(&f.agent).agent_logs.remove(0);
    let kind = match kind {
        AgentLogKind::WorkStepFailed { failure, .. } => {
            AgentLogKind::WorkStepFailed { step, failure }
        }
        _ => unreachable!(),
    };
    let rows: Vec<_> = (f.agent.conversation.last_seq + 1..=10_000)
        .map(|seq| {
            log_document(&AgentLogEntry {
                agent_id: f.agent.id.clone(),
                seq,
                appended_at: MINUTE.into(),
                kind: kind.clone(),
            })
            .unwrap()
        })
        .collect();
    for batch in rows.chunks(200) {
        f.store.agent_log.insert_many(batch).await.unwrap();
    }
    f.agent.conversation.last_seq = 10_000;
    f.agent.revision += 1;
    let mut fields = agent_replacement_fields(&f.agent).unwrap();
    fields.insert("last_seq", 10_000_i64);
    f.change(doc! {"$set": fields, "$unset": {INTEGRITY_FIELD: ""}})
        .await;
    let mut rejected = retry(&f.agent);
    rejected.agent_logs = vec![kind];
    assert_eq!(
        f.store.commit_agent_work(rejected).await,
        Err(SandboxStoreError::AgentLogCapacityExceeded)
    );
    let marker = proposal(&f, MINUTE, TWO_MINUTES).await;
    assert_eq!(marker.agent_logs.len(), 1);
    f.store.commit_agent_work(marker).await.unwrap();
    f.agent = claim(&f.store, &f.agent.id, TWO_MINUTES).await;
    assert_eq!(f.agent.conversation.last_seq, 10_001);
    assert!(
        parse_integrity(&f.row().await)
            .unwrap()
            .unwrap()
            .semantic
            .valid
    );
    f
}

async fn proposal(f: &Fixture, at: &str, due: &str) -> AgentWorkMutation {
    let tail = f
        .store
        .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .unwrap();
    plan_agent_cleanup_capacity_retry(
        &f.agent,
        tail.as_ref(),
        "worker",
        f.agent.work.lease.as_ref().unwrap().fence,
        at,
        due,
    )
    .unwrap()
}

fn decision(metrics: &str, step: AgentWorkStep, name: &str, count: u64) {
    let line = metrics
        .lines()
        .find(|line| {
            line.starts_with("cma_agent_capacity_settlement_reads_total{")
                && line.contains(&format!("worker_step=\"{}\"", step.operation_name()))
                && line.contains(&format!("decision=\"{name}\""))
        })
        .unwrap_or_else(|| panic!("missing {name}: {metrics}"));
    assert!(line.ends_with(&format!(" {count}")), "{line}");
}

async fn warm(f: &mut Fixture) {
    f.store
        .commit_agent_work(proposal(f, TWO_MINUTES, THIRD).await)
        .await
        .unwrap();
    f.agent = claim(&f.store, &f.agent.id, THIRD).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn capacity_settlement_10001_rows_matches_full_snapshot_with_bounded_reads() {
    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
    ] {
        for conflict in [false, true] {
            let mut f = seeded(step).await;
            let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let metrics = recorder.handle();
            let admin =
                Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
                    .await
                    .unwrap();
            let mut total_scans = [0; 4];
            for attempt in 0..3 {
                let seconds = seconds_from_rfc3339(TWO_MINUTES).unwrap() + attempt * 60;
                let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds));
                let due = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds + 60));
                if attempt > 0 {
                    f.agent = claim(&f.store, &f.agent.id, &at).await;
                }
                let mutation = proposal(&f, &at, &due).await;
                assert!(mutation.agent_logs.is_empty());
                let original = f.row().await;
                f.store = f.store.with_bounded_close_retries(false);
                let baseline = reads::isolated_test(async {
                    f.store.commit_agent_work(mutation.clone()).await.unwrap();
                    reads::snapshot(AGENT_LOG)
                })
                .await;
                let expected = f.row().await;
                f.reset(original).await;
                f.store = f.store.with_bounded_close_retries(true);
                if conflict {
                    admin.database("admin").run_command(doc! {"configureFailPoint": "failCommand",
                        "mode": {"times": 1}, "data": {"failCommands": ["update"], "appName": &f.app,
                        "errorCode": 112, "errorLabels": ["TransientTransactionError"]}}).await.unwrap();
                }
                let before_attempts = f.attempts.load(Ordering::SeqCst);
                let observation = metrics::set_default_local_recorder(&recorder);
                let (scan, bounded) = scans::isolated_test(reads::isolated_test(async {
                    f.store.commit_agent_work(mutation).await.unwrap();
                    (
                        scans::snapshot(ScanCaller::CapacitySettlement(step), true),
                        reads::snapshot(AGENT_LOG),
                    )
                }))
                .await;
                drop(observation);
                assert_eq!(
                    f.attempts.load(Ordering::SeqCst) - before_attempts,
                    if conflict { 2 } else { 1 }
                );
                assert_eq!(scan[0], u64::from(attempt == 0));
                if attempt > 0 {
                    assert_eq!(
                        bounded[2], 2,
                        "tail proof plus exact marker, independent of history/conflicts"
                    );
                    assert!(baseline[2] >= 10_001);
                    assert!(bounded[3] < baseline[3] / 100);
                }
                assert_eq!(
                    f.row().await,
                    expected,
                    "same Agent and integrity at {step:?}/{attempt}"
                );
                for (total, value) in total_scans.iter_mut().zip(scan) {
                    *total += value;
                }
                eprintln!("capacity_settlement step={step:?} conflict={conflict} retry={attempt} full_documents={} bounded_documents={} full_bytes={} bounded_bytes={} scans={}",
                    baseline[2], bounded[2], baseline[3], bounded[3], scan[0]);
            }
            assert_eq!(total_scans[0], 1);
            assert_eq!(total_scans[1], 10_001);
            assert_eq!(
                f.store
                    .agent_log
                    .count_documents(doc! {"agent_id": f.agent.id.as_str()})
                    .await
                    .unwrap(),
                10_001
            );
            decision(&metrics.render(), step, "audit_due", 1);
            decision(&metrics.render(), step, "proof", 2);
            if conflict {
                decision(&metrics.render(), step, "conflict_reuse", 3);
            }
            f.cleanup().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn capacity_settlement_gates_restart_expiry_and_untrusted_proofs_scan() {
    for mode in [
        "off",
        "cohort_off",
        "restart",
        "expiry",
        "missing",
        "stale",
        "unsupported",
        "untrusted",
    ] {
        let mut f = seeded(AgentWorkStep::CloseProcess).await;
        warm(&mut f).await;
        match mode {
            "off" => f.store = f.store.with_bounded_close_retries(false),
            "cohort_off" => f.store.bounded_admission_agents.clear(),
            "restart" => {
                f.store = MongoSandboxStore::open(&f.client, &f.name)
                    .await
                    .unwrap()
                    .with_fleet_log_percentage(100)
                    .with_bounded_close_retries(true)
            }
            "expiry" => f.store.retry_audits.expire(&f.agent),
            "missing" => f.change(doc! {"$unset": {INTEGRITY_FIELD: ""}}).await,
            "stale" => {
                f.change(doc! {"$inc": {"agent_log_integrity_v1.bound_agent_revision": -1_i64}})
                    .await
            }
            "unsupported" => {
                f.change(doc! {"$set": {"agent_log_integrity_v1.format": 99_i64}})
                    .await
            }
            "untrusted" => {
                f.change(
                    doc! {"$set": {"agent_log_integrity_v1.semantic_checkpoint_v1.valid": false}},
                )
                .await
            }
            _ => unreachable!(),
        }
        let mutation = proposal(&f, THIRD, FOURTH).await;
        let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
        let metrics = recorder.handle();
        let observation = metrics::set_default_local_recorder(&recorder);
        scans::isolated_test(async {
            f.store.commit_agent_work(mutation.clone()).await.unwrap();
            assert_eq!(
                scans::snapshot(ScanCaller::CapacitySettlement(f.agent.work.step), true)[0],
                1,
                "{mode}"
            );
        })
        .await;
        drop(observation);
        assert_eq!(
            f.store.get_agent(&f.agent.id).await.unwrap(),
            Some(mutation.agent)
        );
        let reason = match mode {
            "off" => "flag_disabled",
            "cohort_off" => "cohort_excluded",
            "restart" | "expiry" => "audit_due",
            _ => "proof_fallback",
        };
        decision(&metrics.render(), f.agent.work.step, reason, 1);
        f.cleanup().await;
    }
}
