//! Diagnostic workload for #1792. Timing results describe the local fixture only.
use super::*;
use layer2_core::sandbox::{AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder};
use layer3_adapter::{ContentBlock, ContentChunk, ConversationUpdate, MessageId, SessionUpdate};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

#[path = "targeted_control_reproduction_writers.rs"]
mod writers;

#[derive(Default)]
struct Observation {
    attempts: u64,
    conflicts: BTreeMap<String, u64>,
    pending: BTreeMap<i32, String>,
}

impl Observation {
    fn event(&mut self, event: CommandEvent) {
        match event {
            CommandEvent::Started(event) => {
                if matches!(event.command.get_bool("startTransaction"), Ok(true)) {
                    self.attempts += 1;
                }
                let collection = event
                    .command
                    .get_str(&event.command_name)
                    .unwrap_or("transaction");
                self.pending.insert(event.request_id, collection.to_owned());
            }
            CommandEvent::Failed(event) => {
                if let Some(collection) = self.pending.remove(&event.request_id) {
                    if is_transient(&event.failure) {
                        *self.conflicts.entry(collection).or_default() += 1;
                    }
                }
            }
            CommandEvent::Succeeded(event) => {
                if let Some(collection) = self.pending.remove(&event.request_id) {
                    let conflict = event.reply.get_array("writeErrors").is_ok_and(|errors| {
                        errors.iter().any(|error| {
                            error
                                .as_document()
                                .is_some_and(|error| matches!(error.get_i32("code"), Ok(112)))
                        })
                    });
                    if conflict {
                        *self.conflicts.entry(collection).or_default() += 1;
                    }
                }
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.attempts = 0;
        self.conflicts.clear();
    }
}

#[derive(Default)]
struct Samples {
    durations_us: Vec<u64>,
    attempts: BTreeMap<u64, u64>,
    outcomes: BTreeMap<&'static str, u64>,
    conflicts: BTreeMap<String, u64>,
}

impl Samples {
    fn record<T>(
        &mut self,
        elapsed: Duration,
        result: &Result<T, CommandLedgerError>,
        observation: &Observation,
    ) {
        self.durations_us.push(elapsed.as_micros() as u64);
        *self.attempts.entry(observation.attempts).or_default() += 1;
        let outcome = match result {
            Ok(_) => "committed",
            Err(CommandLedgerError::Conflict) => "conflict",
            Err(other) => other.code(),
        };
        *self.outcomes.entry(outcome).or_default() += 1;
        for (collection, count) in &observation.conflicts {
            *self.conflicts.entry(collection.clone()).or_default() += count;
        }
    }

    fn report(
        &mut self,
        hz: u64,
        mode: &str,
        writes: u64,
        failures: &BTreeMap<&str, u64>,
        kind: writers::Kind,
    ) {
        self.durations_us.sort_unstable();
        let percentile =
            |p: usize| self.durations_us[(self.durations_us.len() * p).div_ceil(100) - 1];
        eprintln!(
            "{}",
            serde_json::json!({
                "experiment": "1792_competing_writer", "writer": format!("{kind:?}"), "configured_hz": hz, "mode": mode,
                "samples": self.durations_us.len(), "p50_us": percentile(50),
                "p95_us": percentile(95), "p99_us": percentile(99),
                "attempt_histogram": self.attempts, "outcomes": self.outcomes,
                "transient_failures_by_collection": self.conflicts,
                "writer_commits": writes, "writer_errors": failures,
            })
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_control_quiet_checkpoint_reproduction() {
    reproduce(writers::Kind::Checkpoint).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_control_message_admission_reproduction() {
    reproduce(writers::Kind::Admission).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn response_control_queued_cancellation_reproduction() {
    reproduce(writers::Kind::QueuedCancellation).await;
}

async fn reproduce(kind: writers::Kind) {
    for hz in [5, 10, 20] {
        let (client, name) = database("control_checkpoint_reproduction").await;
        let (writer, agent, mut template, _) = composed(&name, &client).await;
        let agent = writers::attach_output(&writer, &agent).await;
        template.command.expected_agent_revision = agent.revision;
        template.observed_agent_revision = agent.revision;
        let before = writer.get_agent(&agent.id).await.unwrap().unwrap();
        let observed = Arc::new(Mutex::new(Observation::default()));
        let handler = observed.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            handler.lock().unwrap().event(event);
        }));
        let control_client = Client::with_options(options).unwrap();
        let mut control = MongoSandboxStore::open(&control_client, &name)
            .await
            .unwrap();
        // A separate client isolates measurements; scheduling belongs to the
        // same process/store instance as the real recorder and control service.
        control.agent_writes = writer.agent_writes.clone();
        let stopped = Arc::new(AtomicBool::new(false));
        let commits = Arc::new(AtomicU64::new(0));
        let failures = Arc::new(Mutex::new(BTreeMap::new()));
        let competing = writers::Writer::new(kind, writer.clone(), agent.clone(), &template).await;
        let task = {
            let stopped = stopped.clone();
            let commits = commits.clone();
            let failures = failures.clone();
            tokio::spawn(async move {
                let mut competing = competing;
                let mut index = 0;
                let mut interval = tokio::time::interval(Duration::from_millis(1000 / hz));
                interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                while !stopped.load(Ordering::Relaxed) {
                    interval.tick().await;
                    match competing.tick(index).await {
                        Ok(()) => {
                            commits.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(code) => {
                            *failures.lock().unwrap().entry(code).or_insert(0) += 1;
                        }
                    }
                    index += 1;
                }
            })
        };
        let mut inspection = Samples::default();
        let mut reservation = Samples::default();
        let mut interval = tokio::time::interval(Duration::from_millis(50));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        for index in 0..100 {
            interval.tick().await;
            let mut request = template.clone();
            request.command.key_sha256 = command_key_digest(&format!("control-{index}")).unwrap();
            request.command.envelope.command_id = AgentCommandId::generate();
            request.command.queue_capacity = MAX_COMMAND_QUEUE_CAPACITY;
            request.retry_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(5));
            observed.lock().unwrap().reset();
            let started = tokio::time::Instant::now();
            let result = control.inspect_targeted_control(request.clone()).await;
            inspection.record(started.elapsed(), &result, &observed.lock().unwrap());
            assert!(
                matches!(result, Ok(_) | Err(CommandLedgerError::Conflict)),
                "unexpected inspection refusal: {result:?}"
            );
            if let Ok(result) = result {
                assert_eq!(result, TargetedControlInspection::Missing);
            }
            observed.lock().unwrap().reset();
            let started = tokio::time::Instant::now();
            let result = control.reserve_targeted_control(request).await;
            reservation.record(started.elapsed(), &result, &observed.lock().unwrap());
            assert!(
                matches!(result, Ok(_) | Err(CommandLedgerError::Conflict)),
                "unexpected reservation refusal: {result:?}"
            );
            if let Ok(accepted) = result {
                // Settle each unexecuted diagnostic control so bounded queue
                // capacity cannot masquerade as contention at later samples.
                writer
                    .reject_unclaimed_command(
                        &accepted.record.receipt.envelope.command_id,
                        accepted.record.revision,
                        CommandFailureCode::TargetTurnChanged,
                        layer2_core::now_rfc3339(),
                    )
                    .await
                    .unwrap();
            }
        }
        stopped.store(true, Ordering::Relaxed);
        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap();
        let writes = commits.load(Ordering::Relaxed);
        {
            let failures = failures.lock().unwrap();
            inspection.report(hz, "inspect", writes, &failures, kind);
            reservation.report(hz, "reserve", writes, &failures, kind);
        }
        assert!(writes > 0, "competing writer must make progress");
        assert_eq!(writer.get_agent(&agent.id).await.unwrap().unwrap(), before);
        client.database(&name).drop().await.unwrap();
    }
}
