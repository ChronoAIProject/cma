//! Opt-in continuity proof with actual runtime/ACP/Codex and disposable Mongo.
//! Kept outside the Mongo-only CI filter: the pinned native tools are required.
use std::sync::atomic::Ordering;
use std::time::Duration;

use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::agent_io::*;
use layer2_core::sandbox::store::{AgentLogStore, AgentStore};
use layer2_core::sandbox::{
    AgentLogKind, AgentMessageAssembly, AgentUpdateRecorder, DurableAgentUpdateRecorder,
};
use mongodb::bson::doc;
use tokio::time::Instant;

use super::sandbox_mongo::MongoSandboxStore;
use super::sandbox_mongo_integration_tests::command_ledger_tests::{
    execution_tests::composed_with_input, record, work_tests::live_request,
};
use crate::worker_supervision::{Activity, Workers};

mod native;
use native::Native;

async fn wait_state(activity: &Activity, expected: &str) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while activity.snapshot().state != expected {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn completed(store: &MongoSandboxStore, id: &AgentCommandId) -> AdmittedAgentCommand {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let command = store.get_command(id).await.unwrap().unwrap();
            assert!(command.work.failure.is_none(), "{command:?}");
            if command.status == AgentResponseStatus::Completed {
                break command;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

async fn recover(stall: bool) {
    let mut native = Native::start().await;
    let client = mongodb::Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let database_name = format!("cma_native_supervision_{}", uuid::Uuid::new_v4().simple());
    let (store, agent, parent, execution, sends) = composed_with_input(
        &client,
        &database_name,
        native.channel.input.clone(),
        native.channel.conversation_id.clone().unwrap(),
    )
    .await;
    let original_conversation = agent.conversation.conversation_id.clone().unwrap();
    let original_status = native.status().await;
    assert_eq!(original_status.agent_processes, 1);
    let dirty = native.cwd.join("dirty-owner-work.txt");
    tokio::fs::write(&dirty, b"unfinished owner work\n")
        .await
        .unwrap();
    let home_sentinel = native.agent_home.join("owner-home-sentinel.txt");
    tokio::fs::write(&home_sentinel, b"retained agent home\n")
        .await
        .unwrap();
    let home_config = tokio::fs::read(native.agent_home.join("config.toml"))
        .await
        .unwrap();
    let home = native.agent_home.clone();
    let home_inode = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&home).unwrap().ino()
    };
    // Consume actual production transport updates, including the real terminal event.
    // No fabricated TurnEnded or manual reconcile_command call is used here.
    let (_, placeholder) = tokio::sync::mpsc::channel(1);
    let mut updates = std::mem::replace(&mut native.updates, placeholder);
    let recorder = DurableAgentUpdateRecorder::new(store.clone());
    let recorded_agent = agent.id.clone();
    let worker =
        AgentCommandWorker::new(store.clone(), execution, "native-supervised-replica".into())
            .unwrap();
    let workers = Workers::new();
    let activity = workers.get("agent_commands");
    let (stop, signal) = tokio::sync::watch::channel(false);
    let reservation = live_request(&agent, &parent, "native-original");
    let original = record(store.reserve_command(reservation.clone()).await.unwrap());
    let recording = tokio::spawn(async move {
        let mut assembly = AgentMessageAssembly::default();
        while let Some(update) = updates.recv().await {
            recorder
                .record_update(
                    &recorded_agent,
                    1,
                    &mut assembly,
                    update,
                    layer2_core::now_rfc3339(),
                )
                .await
                .unwrap();
        }
    });
    let scheduler = crate::commands::CommandScheduler::new(worker.clone())
        .spawn_supervised(signal, activity.clone());
    let id = &original.receipt.envelope.command_id;
    let started = tokio::time::timeout(Duration::from_secs(40), async {
        loop {
            let observed = native.state().await;
            if observed["started"] == true {
                break observed;
            }
            assert!(!recording.is_finished(), "runtime recorder failed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(started["effects"], 1);
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    let claimed = store.get_command(id).await.unwrap().unwrap();
    let dispatch = claimed
        .work
        .dispatch
        .clone()
        .expect("actual runtime dispatch bound");
    let fault = Instant::now();
    if stall {
        crate::worker_supervision::stall_next_poll(&activity);
    } else {
        crate::worker_supervision::panic_next_poll(&activity);
    }
    wait_state(&activity, "backoff").await;
    let detection = fault.elapsed();
    assert!(detection < Duration::from_secs(10));
    assert!(!activity.snapshot().available());
    wait_state(&activity, "running").await;
    let scheduling = fault.elapsed();
    let retained = native.state().await;
    assert_eq!(retained["effects"], 1);
    assert_eq!(retained["task_pid"], started["task_pid"]);
    assert_eq!(retained["task_alive"], true);
    assert!(retained["task_result"].is_null());
    assert_eq!(native.status().await.boot_id, original_status.boot_id);
    assert_eq!(
        store.get_command(id).await.unwrap().unwrap().work.dispatch,
        Some(dispatch.clone())
    );
    native.release("task").await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while native.state().await["thinking"] != true {
            assert!(!recording.is_finished(), "runtime recorder failed");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    // A native turn remains pending beyond the watchdog (both cases) and
    // the sixty-second failure window (panic case), without another restart.
    let quiet = Instant::now();
    let waiting_seconds = if stall { 10 } else { 65 };
    while quiet.elapsed() < Duration::from_secs(waiting_seconds) {
        assert_eq!(activity.snapshot().state, "running");
        assert!(!recording.is_finished(), "runtime recorder failed");
        let current = store.get_command(id).await.unwrap().unwrap();
        assert_eq!(current.status, AgentResponseStatus::InProgress);
        assert_eq!(current.work.dispatch, Some(dispatch.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let released_at = Instant::now();
    native.release("model").await;
    let done = completed(&store, id).await;
    let work = released_at.elapsed();
    assert!(work < Duration::from_secs(30));
    assert_eq!(done.receipt, original.receipt);
    assert_eq!(done.work.dispatch, Some(dispatch));

    let ready = store.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(
        ready.conversation.conversation_id.as_deref(),
        Some(original_conversation.as_str())
    );
    let mut next = live_request(&ready, &parent, "native-continuation");
    next.envelope.command =
        AgentCommand::message("Continue the retained conversation. What was the original result?");
    next.envelope.request_fingerprint = next.envelope.command.fingerprint().unwrap();
    let next_reservation = next.clone();
    let next = record(store.reserve_command(next).await.unwrap());
    completed(&store, &next.receipt.envelope.command_id).await;
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    let final_status = native.status().await;
    assert_eq!(final_status, original_status);
    assert_eq!(
        tokio::fs::read(&dirty).await.unwrap(),
        b"unfinished owner work\n"
    );
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(std::fs::metadata(&home).unwrap().ino(), home_inode);
    }
    assert_eq!(
        tokio::fs::read(&home_sentinel).await.unwrap(),
        b"retained agent home\n"
    );
    assert_eq!(
        tokio::fs::read(home.join("config.toml")).await.unwrap(),
        home_config
    );
    let observed = native.state().await;
    assert_eq!(observed["effects"], 1);
    assert_eq!(observed["task_result"], "completed\n");
    use layer3_adapter::hosted::HostedControlTransport;
    let history = native
        .transport
        .export_agent_history(
            &native.handle,
            &layer3_adapter::AgentSlot::new(agent.id.as_str()),
        )
        .await
        .unwrap();
    let history_text = String::from_utf8(history.clone()).unwrap();
    assert!(history_text.contains(&original_conversation));
    assert!(history_text.contains("ORIGINAL_RUNTIME_RESULT"));
    assert!(history_text.contains("CONTINUED_SAME_CONVERSATION"));
    tokio::fs::write(
        std::path::Path::new(native.manifest["root"].as_str().unwrap()).join("history.ndjson"),
        history,
    )
    .await
    .unwrap();
    stop.send(true).unwrap();
    scheduler.await.unwrap();
    assert_eq!(activity.snapshot().state, "stopped");
    let reopened = MongoSandboxStore::open(&client, &database_name)
        .await
        .unwrap();
    assert_eq!(
        record(reopened.reserve_command(reservation).await.unwrap()),
        done
    );
    record(reopened.reserve_command(next_reservation).await.unwrap());
    worker.run_one(id).await.unwrap();
    worker
        .run_one(&next.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 2);
    assert_eq!(
        reopened
            .agent_commands
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    let logs = reopened.agent_log(&agent.id, 0, 100).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|log| matches!(log.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        2
    );
    let rendered = serde_json::to_string(&logs).unwrap();
    assert!(rendered.contains("ORIGINAL_RUNTIME_RESULT"));
    assert!(rendered.contains("CONTINUED_SAME_CONVERSATION"));
    let evidence = serde_json::json!({"passed":true,"stall":stall,"detection_ms":detection.as_millis(),"scheduling_ms":scheduling.as_millis(),"healthy_native_wait_seconds":waiting_seconds,"completion_after_model_release_ms":work.as_millis(),"completion_after_fault_ms":fault.elapsed().as_millis(),"runtime":final_status,"conversation_id":original_conversation,"native_shell_effects":1,"durable_commands":2,"dispatches":2,"state":observed});
    tokio::fs::write(
        std::path::Path::new(native.manifest["root"].as_str().unwrap()).join("result.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .await
    .unwrap();
    println!("NATIVE_SUPERVISION_RESULT={evidence}");
    native.close().await;
    recording.abort();
    client.database(&database_name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable Mongo and pinned real runtime/Codex/ACP; see worker-runtime-supervision.sh"]
async fn scheduler_panic_preserves_native_task_home_and_conversation() {
    tokio::time::timeout(Duration::from_secs(240), recover(false))
        .await
        .expect("native continuity deadline");
}

#[tokio::test]
#[ignore = "requires disposable Mongo and pinned real runtime/Codex/ACP; see worker-runtime-supervision.sh"]
async fn scheduler_stall_preserves_native_task_home_and_conversation() {
    tokio::time::timeout(Duration::from_secs(240), recover(true))
        .await
        .expect("native continuity deadline");
}
