use super::*;
use layer2_core::{rfc3339_at, seconds_from_rfc3339};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn repeated_failure_scan_baseline() {
    for step in [
        AgentWorkStep::CloseProcess,
        AgentWorkStep::EnsureProcessClosed,
        AgentWorkStep::RemoveHome,
        AgentWorkStep::RemoveWorktree,
    ] {
        for conflict in [false, true] {
            let mut f = Fixture::new(1_000).await;
            position(&mut f, step).await;
            let initial_head = f.agent.conversation.last_seq;
            let caller = ScanCaller::WorkerPublication(step);
            f.store = f.store.with_bounded_close_retries(false);
            let admin =
                Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap())
                    .await
                    .unwrap();
            let mut totals = [0; 4];
            let mut checkouts = 0;
            let mut commands = 0;
            let mut attempts = 0;
            let mut elapsed = 0;
            let mut latencies = Vec::new();
            for attempt in 0..5 {
                let at = seconds_from_rfc3339(MINUTE).unwrap() + attempt * 60;
                let at = rfc3339_at(UNIX_EPOCH + Duration::from_secs(at));
                if attempt > 0 {
                    f.agent = claim(&f.store, &f.agent.id, &at).await;
                }
                let mut mutation = retry(&f.agent);
                if let AgentLogKind::WorkStepFailed { step: logged, .. } =
                    &mut mutation.agent_logs[0]
                {
                    *logged = step;
                }
                mutation.appended_at = at.clone();
                mutation.agent.updated_at = at;
                mutation.agent.work.ready_at = rfc3339_at(
                    UNIX_EPOCH
                        + Duration::from_secs(
                            seconds_from_rfc3339(MINUTE).unwrap() + (attempt + 1) * 60,
                        ),
                );
                if conflict && attempt > 0 {
                    admin.database("admin").run_command(doc! { "configureFailPoint": "failCommand", "mode": { "times": 1 },
                    "data": { "failCommands": ["update"], "appName": &f.app, "errorCode": 112, "errorLabels": ["TransientTransactionError"] }
                }).await.unwrap();
                }
                let start_checkouts = f.checkouts.load(Ordering::SeqCst);
                let start_commands = f.commands.load(Ordering::SeqCst);
                let start_attempts = f.attempts.load(Ordering::SeqCst);
                let start = Instant::now();
                let scan = scans::isolated_test(async {
                    f.store.commit_agent_work(mutation).await.unwrap();
                    scans::snapshot(caller, true)
                })
                .await;
                if attempt > 0 {
                    let micros = start.elapsed().as_micros();
                    elapsed += micros;
                    latencies.push(micros);
                    commands += f.commands.load(Ordering::SeqCst) - start_commands;
                    attempts += f.attempts.load(Ordering::SeqCst) - start_attempts;
                    checkouts += f.checkouts.load(Ordering::SeqCst) - start_checkouts;
                    for (total, value) in totals.iter_mut().zip(scan) {
                        *total += value;
                    }
                }
            }
            let saved = f.store.get_agent(&f.agent.id).await.unwrap().unwrap();
            assert_eq!(saved.conversation.last_seq, initial_head + 5);
            assert_eq!(saved.work.attempts, 5);
            assert_eq!(totals[0], if conflict { 8 } else { 4 });
            assert_eq!(attempts, if conflict { 8 } else { 4 });
            latencies.sort_unstable();
            let p50_us = latencies[1];
            let p95_us = latencies[3];
            eprintln!("retry_baseline step={step:?} conflict={conflict} repeated_publications=4 commands={commands} attempts={attempts} inferred_attempts={} scans={} documents={} bytes={} checkouts={checkouts} elapsed_us={elapsed} p50_us={p50_us} p95_us={p95_us} added_lines=4 total_lines_including_first=5", totals[0], totals[0], totals[1], totals[2]);
            f.cleanup().await;
        }
    }
}

mod cleanup_fixture;
mod contracts;
use cleanup_fixture::position;

mod integrity;
mod segmented;

mod lifecycle;

mod conflict_invalidation;
