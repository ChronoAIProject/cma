//! Manual measurement of the real claim/failure transaction, without provider I/O.
use super::*;
use mongodb::{
    event::{cmap::CmapEvent, EventHandler},
    options::ClientOptions,
};

#[tokio::test]
#[ignore = "manual CPU profile; requires an owned disposable replica set"]
async fn cleanup_retry_profile() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let captured = count.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            captured.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_cpu_cleanup_{}", uuid::Uuid::new_v4().simple());
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let mut h = Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
    h.now = SystemTime::now();
    h.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: h.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: h.registry.clone(),
            updates: h.updates.clone(),
            operations: AgentOperationService::new(store, h.registry.clone()),
        },
        AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID),
    );
    let agent = h.admit("cleanup-cpu", vec![]).await;
    for _ in 0..5 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let ready = h.stored(&agent.id).await;
    h.store
        .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: AgentStopReason::Owner,
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    for _ in 0..3 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    let stopped = h.stored(&agent.id).await;
    h.store
        .schedule_agent_delete(AgentDeleteRequest {
            target: AgentControlTarget::from(&stopped),
            requested_at: rfc3339_at(SystemTime::now()),
        })
        .await
        .unwrap();
    for _ in 0..2 {
        h.run(&agent.id, SystemTime::now()).await;
    }
    assert_eq!(
        h.stored(&agent.id).await.work.step,
        AgentWorkStep::RemoveWorktree
    );
    let mut now = SystemTime::now();
    for large in [false, true] {
        if large {
            fill_history(&client, &name, &h, &agent.id).await;
        }
        let before = count.load(Ordering::SeqCst);
        let cpu = cpu_seconds();
        let started = std::time::Instant::now();
        for _ in 0..30 {
            h.backend.fail_next(
                WireOperation::RemoveWorktree,
                "mutation_authority_corrupt",
                "fixture authority rejected",
            );
            h.run(&agent.id, now).await;
            let current = h.stored(&agent.id).await;
            assert_eq!(current.work.step, AgentWorkStep::RemoveWorktree);
            now = SystemTime::UNIX_EPOCH
                + Duration::from_secs(
                    layer2_core::seconds_from_rfc3339(&current.work.ready_at).unwrap() + 1,
                );
        }
        let elapsed = started.elapsed().as_secs_f64();
        let consumed = cpu_seconds() - cpu;
        let checkouts = count.load(Ordering::SeqCst) - before;
        println!(
            "CPU_PROFILE {}",
            serde_json::json!({"phase": if large {"cleanup_retry_10000_entries"} else {"cleanup_retry_short_history"},
            "iterations":30,"wall_seconds":elapsed,"cpu_seconds":consumed,"mean_cores":consumed/elapsed,
            "checkouts":checkouts,"checkouts_per_iteration":checkouts as f64/30.,
            "cpu_ms_per_iteration":consumed*1000./30.,"wall_ms_per_iteration":elapsed*1000./30.})
        );
    }
    // Fixed offered workload: one due-discovery per logical second for one
    // hour. Provider errors are deterministic and instantaneous. Only the
    // persisted retry schedule decides which attempts actually run.
    let base = now;
    let before = count.load(Ordering::SeqCst);
    let cpu = cpu_seconds();
    let started = std::time::Instant::now();
    let mut attempts = 0;
    for second in 0..3600 {
        let at = base + Duration::from_secs(second);
        let due = h.store.due_agent_work(&rfc3339_at(at), 50).await.unwrap();
        if due.contains(&agent.id) {
            h.backend.fail_next(
                WireOperation::RemoveWorktree,
                "mutation_authority_corrupt",
                "fixture authority rejected",
            );
            h.run(&agent.id, at).await;
            attempts += 1;
        }
    }
    let consumed = cpu_seconds() - cpu;
    let checkouts = count.load(Ordering::SeqCst) - before;
    println!(
        "CPU_PROFILE {}",
        serde_json::json!({"phase":"steady_removal_logical_hour_10000_entries",
        "logical_seconds":3600,"attempts":attempts,"cpu_seconds":consumed,"wall_seconds":started.elapsed().as_secs_f64(),
        "checkouts":checkouts,"checkouts_per_logical_second":checkouts as f64/3600.,"cpu_cores_per_logical_second":consumed/3600.})
    );
    client.database(&name).drop().await.unwrap();
}

fn cpu_seconds() -> f64 {
    let result = std::process::Command::new("ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .trim()
        .split(':')
        .fold(0., |sum, part| sum * 60. + part.parse::<f64>().unwrap())
}
