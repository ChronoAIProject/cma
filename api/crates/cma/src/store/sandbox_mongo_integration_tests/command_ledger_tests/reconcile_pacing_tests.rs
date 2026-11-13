use super::*;
use crate::store::agent_log_telemetry::{snapshot, snapshot_all, ScanCaller};
use mongodb::event::{command::CommandEvent, EventHandler};
use mongodb::options::ClientOptions;

const DISCOVERY_POLLS: u64 = 16;

#[derive(Default, Debug)]
struct Measurements {
    enabled: bool,
    commands: std::collections::BTreeMap<String, u64>,
    checkout_us: u128,
    checkouts: u64,
}

async fn measured_client() -> (Client, Arc<Mutex<Measurements>>) {
    let values = Arc::new(Mutex::new(Measurements::default()));
    let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let observed = values.clone();
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            let mut values = observed.lock().unwrap();
            if values.enabled {
                *values.commands.entry(event.command_name).or_default() += 1;
            }
        }
    }));
    let observed = values.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if let mongodb::event::cmap::CmapEvent::ConnectionCheckedOut(event) = event {
            let mut values = observed.lock().unwrap();
            if values.enabled {
                values.checkout_us += event.duration.as_micros();
                values.checkouts += 1;
            }
        }
    }));
    (Client::with_options(options).unwrap(), values)
}

fn scans() -> [u64; 4] {
    let admission = snapshot(ScanCaller::CommandAdmission, true);
    let reconciliation = snapshot(ScanCaller::CommandReconciliation, true);
    std::array::from_fn(|index| admission[index] + reconciliation[index])
}

async fn working(
    client: &Client,
    name: &str,
    count: u64,
    bounded: bool,
) -> (
    Arc<MongoSandboxStore>,
    Agent,
    AdmittedAgentCommand,
    Arc<crate::commands::ResourceCommandExecution>,
) {
    let (store, agent, parent, execution, _, _) = composed_with_history(
        client,
        name,
        Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        active_sandbox("sbx_commands"),
        // Dispatch appends five admission records and one delivery record.
        // Count them inside the measured log so the 10,000-row case can admit
        // the command without crossing the normal history capacity first.
        Some((count - 6, bounded)),
    )
    .await;
    let command = record(
        store
            .reserve_command(live_request(&agent, &parent, "reconcile-pacing"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution.clone(), "dispatch".into())
        .unwrap()
        .run_scheduled(&command.receipt.envelope.command_id)
        .await
        .unwrap();
    let command = store
        .get_command(&command.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap();
    assert!(command.work.dispatch.is_some());
    assert!(!command.status.is_terminal());
    assert_eq!(
        store
            .get_agent(&agent.id)
            .await
            .unwrap()
            .unwrap()
            .conversation
            .last_seq,
        count,
        "measurement includes the durable dispatch and delivery records"
    );
    (store, agent, command, execution)
}

async fn measure(count: u64, enabled: bool) -> (u64, u64) {
    let (client, values) = measured_client().await;
    let name = format!("cma_reconcile_profile_{}", uuid::Uuid::new_v4().simple());
    let (store, agent, command, execution) = working(&client, &name, count, false).await;
    let heads = store
        .command_reconcile_heads(std::slice::from_ref(&agent.id))
        .await
        .unwrap();
    assert_eq!(heads.len(), 1);
    assert!(command.reconciliation_current(&heads[0]));
    let worker = AgentCommandWorker::with_recheck_delay(
        store.clone(),
        execution,
        "measure".into(),
        Duration::from_secs(u64::from(enabled)),
    )
    .unwrap();
    let before = scans();
    let all_before = snapshot_all();
    let reconciliation_before = snapshot(ScanCaller::CommandReconciliation, true);
    values.lock().unwrap().enabled = true;
    let started = std::time::Instant::now();
    let mut scheduler = crate::commands::CommandScheduler::new(worker)
        .with_reconcile_pacing(enabled)
        .driven();
    // Sixteen completed polls cover at least the original four-second window.
    // Counting opportunities, rather than scans that fit in a wall-clock sleep,
    // keeps the OFF baseline independent of the runner's Mongo throughput.
    for _ in 0..DISCOVERY_POLLS {
        scheduler.poll().await;
    }
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_secs(4));
    values.lock().unwrap().enabled = false;
    let after = scans();
    let all_after = snapshot_all();
    let all_delta: [u64; 4] = std::array::from_fn(|index| all_after[index] - all_before[index]);
    let delta: [u64; 4] = std::array::from_fn(|index| after[index] - before[index]);
    let reconciliation =
        snapshot(ScanCaller::CommandReconciliation, true)[0] - reconciliation_before[0];
    if enabled {
        assert_eq!(reconciliation, 1);
    } else {
        assert_eq!(reconciliation, DISCOVERY_POLLS);
    }
    assert_eq!(delta[1], count * reconciliation);
    assert_eq!(
        delta[0], reconciliation,
        "attribution must retain the combined caller total"
    );
    assert_eq!(
        all_delta, delta,
        "all callers must be accounted for in the idle window"
    );
    {
        let measured = values.lock().unwrap();
        let round_trips: u64 = measured.commands.values().sum();
        println!("reconcile_profile rows={count} enabled={enabled} admission_percent=0 N=1 U=1 discovery_polls={DISCOVERY_POLLS} window_s={:.3} combined_scans={} reconciliation_scans={reconciliation} scans_per_s={:.3} documents={} bytes={} scan_us={} round_trips={round_trips} commands={:?} checkout_count={} checkout_us={}",
            elapsed.as_secs_f64(), delta[0], delta[0] as f64 / elapsed.as_secs_f64(), delta[1], delta[2], delta[3], measured.commands, measured.checkouts, measured.checkout_us);
    }
    println!("reconcile_all_callers rows={count} enabled={enabled} scans={} documents={} bytes={} scan_us={}", all_delta[0], all_delta[1], all_delta[2], all_delta[3]);
    // Terminal evidence must wake the same scheduler that has already skipped
    // current heads, so a restart's mandatory fold cannot hide a broken hint.
    if enabled {
        assert!(
            values
                .lock()
                .unwrap()
                .commands
                .get("find")
                .copied()
                .unwrap_or(0)
                >= 11,
            "the measurement must include discovery cycles after the first fold"
        );
    }
    // At normal capacity, native completion needs the exact prompt identity and
    // durable output receipt before it can use the existing stop reserve.
    let dispatch = command.work.dispatch.as_ref().unwrap();
    let terminal = layer3_adapter::ConversationUpdate::TurnEndedForPrompt {
        prompt_id: layer3_adapter::PromptIdentity::new(
            agent.conversation.conversation_id.as_deref().unwrap(),
            dispatch.runtime_generation,
            dispatch.turn_sequence,
        ),
        response: layer3_adapter::PromptResponse::new(layer3_adapter::TurnStopReason::EndTurn),
    };
    let (acks, mut received) = tokio::sync::mpsc::channel(1);
    let terminal = layer3_adapter::ConversationUpdate::RuntimeOutput {
        receipt: layer3_adapter::RuntimeOutputReceipt::new(
            layer3_adapter::RuntimeOutputSource {
                stream_id: uuid::Uuid::new_v4().to_string(),
                cursor: 1,
                native_conversation_id: agent.conversation.conversation_id.clone().unwrap(),
                payload_sha256: "a".repeat(64),
            },
            acks,
        ),
        update: Some(Box::new(terminal).into()),
    };
    let terminal_started = std::time::Instant::now();
    let outcome = DurableAgentUpdateRecorder::new(store.clone())
        .record_update(
            &agent.id,
            1,
            &mut AgentMessageAssembly::default(),
            terminal,
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(outcome.turn_closed);
    assert!(
        received.try_recv().is_err(),
        "the recorder does not ACK for its caller"
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            scheduler.poll().await;
            if store
                .get_command(&command.receipt.envelope.command_id)
                .await
                .unwrap()
                .unwrap()
                .status
                .is_terminal()
            {
                break;
            }
        }
    })
    .await
    .expect("terminal evidence must settle within the five-second budget");
    println!(
        "reconcile_terminal rows={count} enabled={enabled} latency_us={}",
        terminal_started.elapsed().as_micros()
    );
    assert_eq!(
        store
            .get_command(&command.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentResponseStatus::Completed
    );
    client.database(&name).drop().await.unwrap();
    (delta[0], delta[2])
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unchanged_thousand_row_log_has_one_scan_with_pacing() {
    let before = measure(1_000, false).await;
    let after = measure(1_000, true).await;
    assert!(after.0 * 5 <= before.0);
    assert!(after.1 * 5 <= before.1);
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn unchanged_ten_thousand_row_log_has_one_scan_with_pacing() {
    let before = measure(10_000, false).await;
    let after = measure(10_000, true).await;
    assert!(after.0 * 5 <= before.0);
    assert!(after.1 * 5 <= before.1);
}

#[path = "reconcile_pacing_fallback_tests.rs"]
mod fallback;
