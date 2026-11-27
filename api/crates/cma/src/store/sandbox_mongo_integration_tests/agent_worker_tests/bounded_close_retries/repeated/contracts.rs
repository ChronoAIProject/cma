use super::*;
use layer2_core::sandbox::store::{plan_agent_repeated_failure, AgentRepeatedFailure};

pub(super) fn proposal(
    agent: &Agent,
    marker: &AgentLogEntry,
    at: &str,
    due: &str,
) -> AgentRepeatedFailure {
    plan_agent_repeated_failure(
        agent,
        Some(marker),
        "runtime_unavailable",
        "worker",
        agent.work.lease.as_ref().unwrap().fence,
        at,
        due,
    )
    .unwrap()
    .unwrap()
}

pub(super) async fn seeded() -> Fixture {
    seeded_step(AgentWorkStep::CloseProcess).await
}

async fn seeded_step(step: AgentWorkStep) -> Fixture {
    let mut f = Fixture::new(1_000).await;
    position(&mut f, step).await;
    // The first failure still appends. Separate bounded-close activation is
    // switched off here so this measurement includes its original full audit.
    f.store = f.store.with_bounded_close_retries(false);
    let mut first = retry(&f.agent);
    if let AgentLogKind::WorkStepFailed { step: logged, .. } = &mut first.agent_logs[0] {
        *logged = step;
    }
    f.store.commit_agent_work(first).await.unwrap();
    f.agent = claim(&f.store, &f.agent.id, TWO_MINUTES).await;
    f.store = f.store.with_bounded_close_retries(true);
    f
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn repeated_failure_bounds_scans_with_conflicts_and_matches_full_snapshot() {
    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
    ] {
        for conflict in [false, true] {
            let recorder = metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let metrics = recorder.handle();
            let baseline_recorder =
                metrics_exporter_prometheus::PrometheusBuilder::new().build_recorder();
            let baseline_metrics = baseline_recorder.handle();
            let mut f = seeded_step(step).await;
            let initial_head = f.agent.conversation.last_seq;
            let caller = ScanCaller::WorkerPublication(step);
            let marker = f
                .store
                .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
                .await
                .unwrap()
                .unwrap();
            let mut totals = [0; 4];
            let mut checkouts = 0;
            let mut commands = 0;
            let mut attempts = 0;
            let mut elapsed = 0;
            let mut latencies = Vec::new();
            let admin =
                Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
                    .await
                    .unwrap();
            for attempt in 0..4 {
                let seconds = seconds_from_rfc3339(TWO_MINUTES).unwrap() + attempt * 60;
                let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds));
                let due = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds + 60));
                if attempt > 0 {
                    f.agent = claim(&f.store, &f.agent.id, &at).await;
                }
                let retry = proposal(&f.agent, &marker, &at, &due);
                // Compare every persisted Agent and integrity field, including
                // bounded repeats after the first complete audit. Keep reference
                // publications outside the measured interval and audit cache.
                let original = f.row().await;
                f.store = f.store.with_bounded_close_retries(false);
                let observation = metrics::set_default_local_recorder(&baseline_recorder);
                f.store
                    .commit_agent_work_repeated_failure(retry.clone(), None)
                    .await
                    .unwrap();
                drop(observation);
                let expected = f.row().await;
                f.reset(original).await;
                f.store = f.store.with_bounded_close_retries(true);
                if conflict {
                    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 1 },
                    "data": { "failCommands": ["update"], "appName": &f.app, "errorCode": 112, "errorLabels": ["TransientTransactionError"] }
                }).await.unwrap();
                }
                let start_checkouts = f.checkouts.load(Ordering::SeqCst);
                let start_commands = f.commands.load(Ordering::SeqCst);
                let start_attempts = f.attempts.load(Ordering::SeqCst);
                let start = Instant::now();
                let observation = metrics::set_default_local_recorder(&recorder);
                let scan = scans::isolated_test(async {
                    f.store
                        .commit_agent_work_repeated_failure(retry.clone(), None)
                        .await
                        .unwrap();
                    scans::snapshot(caller, true)
                })
                .await;
                drop(observation);
                let micros = start.elapsed().as_micros();
                elapsed += micros;
                latencies.push(micros);
                commands += f.commands.load(Ordering::SeqCst) - start_commands;
                attempts += f.attempts.load(Ordering::SeqCst) - start_attempts;
                checkouts += f.checkouts.load(Ordering::SeqCst) - start_checkouts;
                assert_eq!(scan[0], u64::from(attempt == 0));
                for (total, value) in totals.iter_mut().zip(scan) {
                    *total += value;
                }
                assert_eq!(
                    f.store.get_agent(&f.agent.id).await.unwrap(),
                    Some(retry.mutation.agent)
                );
                assert_eq!(f.row().await, expected);
            }
            let saved = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
            assert_eq!(saved.conversation.last_seq, initial_head);
            assert_eq!(saved.work.attempts, 5);
            assert_eq!(
                f.store
                    .agent_log
                    .count_documents(doc! {"agent_id": f.agent.id.as_str()})
                    .await
                    .unwrap(),
                initial_head
            );
            assert_eq!(attempts, if conflict { 8 } else { 4 });
            let rendered = metrics.render();
            let baseline_rendered = baseline_metrics.render();
            for (text, mode, expected) in
                [(&baseline_rendered, "off", 4), (&rendered, "on", attempts)]
            {
                let counter = text.lines().find(|line| {
                    line.starts_with("cma_agent_close_retry_eligible_attempts_total{")
                        && line.contains(&format!("mode=\"{mode}\""))
                        && line.contains("cohort=\"included\"")
                });
                if step == AgentWorkStep::CloseProcess {
                    let line = counter.expect("eligible CloseProcess attempts must be exported");
                    assert!(line.ends_with(&format!(" {expected}")), "{line}");
                } else {
                    assert!(counter.is_none(), "unrelated step {step:?}");
                }
            }
            for (outcome, expected) in [
                ("completed_full", 1),
                ("skipped_proof", 3),
                ("skipped_conflict_reuse", if conflict { 4 } else { 0 }),
            ] {
                let line = rendered.lines().find(|line| {
                    line.starts_with("cma_agent_retry_audits_total{")
                        && line.contains(&format!("outcome=\"{outcome}\""))
                });
                if expected == 0 {
                    assert!(line.is_none());
                } else {
                    let line = line.expect("retry verification counter must be exported");
                    assert!(line.contains(&format!("worker_step=\"{}\"", step.operation_name())));
                    assert!(line.ends_with(&format!(" {expected}")), "{line}");
                }
            }
            latencies.sort_unstable();
            let p50_us = latencies[1];
            let p95_us = latencies[3];
            eprintln!("retry_single_marker step={step:?} conflict={conflict} repeated_publications=4 commands={commands} attempts={attempts} inferred_attempts={} scans={} documents={} bytes={} checkouts={checkouts} elapsed_us={elapsed} p50_us={p50_us} p95_us={p95_us} added_lines=0", if conflict {8} else {4}, totals[0], totals[1], totals[2]);
            f.cleanup().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn repeated_failure_restart_expiry_rollback_and_class_change() {
    let mut f = seeded().await;
    let marker = f
        .store
        .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .unwrap()
        .unwrap();
    for attempt in 0..5 {
        let seconds = seconds_from_rfc3339(TWO_MINUTES).unwrap() + attempt * 60;
        let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds));
        let due = rfc3339_at(UNIX_EPOCH + Duration::from_secs(seconds + 60));
        if attempt > 0 {
            f.agent = claim(&f.store, &f.agent.id, &at).await;
        }
        if attempt == 2 {
            f.store.retry_audits.expire(&f.agent);
        } else if attempt == 3 {
            f.store = MongoSandboxStore::open(&f.client, &f.name)
                .await
                .unwrap()
                .with_fleet_log_percentage(100)
                .with_bounded_close_retries(true);
        } else if attempt == 4 {
            f.store = f.store.with_bounded_close_retries(false);
        }
        scans::isolated_test(async {
            f.store
                .commit_agent_work_repeated_failure(proposal(&f.agent, &marker, &at, &due), None)
                .await
                .unwrap();
            assert_eq!(scans::snapshot(CALLER, true)[0], u64::from(attempt != 1));
        })
        .await;
    }
    let at = "2026-09-05T00:07:00Z";
    f.agent = claim(&f.store, &f.agent.id, at).await;
    // Rolling back to an ordinary writer resumes appends with no data repair.
    let mut old = retry(&f.agent);
    old.appended_at = at.into();
    old.agent.updated_at = at.into();
    old.agent.work.ready_at = "2026-09-05T00:08:00Z".into();
    f.store.commit_agent_work(old.clone()).await.unwrap();
    f.agent = claim(&f.store, &f.agent.id, "2026-09-05T00:08:00Z").await;
    let tail = f
        .store
        .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .unwrap()
        .unwrap();
    assert!(plan_agent_repeated_failure(
        &f.agent,
        Some(&tail),
        "different_class",
        "worker",
        f.agent.work.lease.as_ref().unwrap().fence,
        "2026-09-05T00:08:00Z",
        "2026-09-05T00:09:00Z"
    )
    .unwrap()
    .is_none());
    let mut changed = retry(&f.agent);
    changed.appended_at = "2026-09-05T00:08:00Z".into();
    changed.agent.updated_at = changed.appended_at.clone();
    changed.agent.work.ready_at = "2026-09-05T00:09:00Z".into();
    if let AgentLogKind::WorkStepFailed { failure, .. } = &mut changed.agent_logs[0] {
        failure.class = "different_class".into();
    }
    f.store.commit_agent_work(changed).await.unwrap();
    f.agent = claim(&f.store, &f.agent.id, "2026-09-05T00:09:00Z").await;
    let tail = f
        .store
        .agent_log_entry(&f.agent.id, f.agent.conversation.last_seq)
        .await
        .unwrap()
        .unwrap();
    let reenabled = plan_agent_repeated_failure(
        &f.agent,
        Some(&tail),
        "different_class",
        "worker",
        f.agent.work.lease.as_ref().unwrap().fence,
        "2026-09-05T00:09:00Z",
        "2026-09-05T00:09:30Z",
    )
    .unwrap()
    .unwrap();
    f.store = f.store.with_bounded_close_retries(true);
    f.store
        .commit_agent_work_repeated_failure(reenabled, None)
        .await
        .unwrap();
    assert_eq!(
        f.store
            .agent_log
            .count_documents(doc! { "agent_id": f.agent.id.as_str() })
            .await
            .unwrap(),
        1_003
    );
    f.cleanup().await;
}
