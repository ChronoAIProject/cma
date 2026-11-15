//! Independent executables sharing one authenticated replica-set database.
//! Provider fixture admission proves this exact fixture contract, not native sends.
use crate::runner_process::capture;
#[path = "../support/replica_delivery.rs"]
mod delivery;
#[path = "replica_queue_oracle.rs"]
mod queue;
#[allow(dead_code)] // Reuse the existing full HTTP snapshot/SSE fixture in this integration binary.
#[path = "../../src/turns/routed/adapter_test_server.rs"]
mod replay_provider;
#[path = "replica_trace.rs"]
mod trace;
#[path = "replica_workload.rs"]
mod workload;

use crate::{
    fake_nyxid_reply::FakeReply,
    runner_process::{database::DatabaseFixture, Process},
    support::{Envelope, Fixture},
};
use cmaeg::ingress::routing_store::EventIdentity;
use cmaeg_core::ids::EventId;
use delivery::{routed_channel, routed_env};
use mongodb::bson::doc;
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path, sync::Arc};

fn ingress_reason(value: &Value) -> &'static str {
    match value["detail"].as_str().or_else(|| value["code"].as_str()) {
        Some("agent_command_key_conflict") => "agent_command_key_conflict",
        Some("event_id_content_conflict") => "event_id_content_conflict",
        Some("provider_busy") => "provider_busy",
        Some("conflict") => "conflict",
        Some("routed_admission_unavailable") => "routed_admission_unavailable",
        Some("routed_authority_changed_retry") => "routed_authority_changed_retry",
        Some("routed_provider_replay_unavailable") => "routed_provider_replay_unavailable",
        Some("custom_durable_admission_unavailable") => "custom_durable_admission_unavailable",
        Some("provider_unavailable") => "provider_unavailable",
        Some("internal") => "internal",
        _ => "unclassified",
    }
}

#[test]
fn failure_diagnostics_reject_unreviewed_text() {
    assert_eq!(
        ingress_reason(&json!({"detail":"routed_admission_unavailable"})),
        "routed_admission_unavailable"
    );
    assert_eq!(
        ingress_reason(&json!({"code":"provider_busy"})),
        "provider_busy"
    );
    assert_eq!(
        ingress_reason(&json!({"detail":"private-payload"})),
        "unclassified"
    );
    assert_eq!(ingress_reason(&json!({"detail":42})), "unclassified");
}

#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo replica set"]
async fn independent_replicas_share_storage_and_preserve_one_provider_admission() {
    for replicas in [1, 2] {
        verify_replicas(replicas, false).await;
        verify_replicas(replicas, true).await;
    }
}

// Keep the inherited N3 coverage available without widening the N1/N2 diagnostic.
#[tokio::test]
#[ignore = "requires authenticated disposable service Mongo replica set"]
async fn three_replicas_share_storage_and_preserve_one_provider_admission() {
    verify_replicas(3, false).await;
    verify_replicas(3, true).await;
}

async fn verify_replicas(replicas: usize, ownership_enabled: bool) {
    let trace = trace::Trace::new(replicas, ownership_enabled);
    trace.stage("cohort", "start", 0);
    verify_cohort(replicas, ownership_enabled, &trace).await;
    // The awaited future and all function-local fixtures have now been dropped.
    trace.stage("teardown", "end", 0);
    trace.stage("cohort", "end", 0);
}

async fn verify_cohort(replicas: usize, ownership_enabled: bool, trace: &trace::Trace) {
    let mut ledger = capture::Ledger::new(replicas, ownership_enabled);
    capture::verify_application();
    ledger.record(json!({"stage":"setup", "unique_events":1}));
    trace.stage("setup", "start", 0);
    let provider = replay_provider::HttpServer::start().await;
    {
        let mut memory = provider.memory.lock().unwrap();
        memory.config.mirror_replay_metadata = true;
        let lifecycle = |seq, kind, status| {
            json!({"type":kind,"sequence_number":seq,
                "response":{"id":"resp_accepted","object":"response","status":status,
                "conversation":{"id":"agt_native"},"metadata":{},"output":[]}})
        };
        memory.config.replay_frames = Some(vec![
            lifecycle(0, "response.created", "queued"),
            json!({"type":"response.output_text.delta","sequence_number":1,"item_id":"m","delta":"hello"}),
            lifecycle(2, "response.completed", "completed"),
        ]);
    }
    let providers = json!([{"slug":"cma_codex","base_url":provider.base,"kind":"responses_http","max_inflight":1,
            "paths":{"agent_card":"/card","binding":"/bindings/{binding_id}","conversation":"/bindings/{binding_id}/conversations/{conversation_id}",
            "responses":"/api/v2/responses","recovery":{"response":"/api/v2/responses/{response_id}","events":"/api/v2/responses/{response_id}/events"}}}]);
    let sink = FakeReply::start(vec![]).await;
    let mut fixture = Fixture::new(routed_channel, Default::default()).await;
    let channel = fixture
        .state
        .resolver
        .resolve(fixture.id.as_str())
        .await
        .unwrap();
    let mut routing = routed_env(channel.pointer.record_id.as_str());
    routing.push((
        "CMAEG_REPLICAS_ENABLED".into(),
        ownership_enabled.to_string(),
    ));
    // Membership shares a deployment identity while each process advertises its own address.
    routing.push(("CMAEG_PUBLIC_URL".into(), "http://127.0.0.1:18999".into()));
    routing.push(("RUST_LOG".into(), "cmaeg=info,cmaeg::ingress::admission_trace=debug,cmaeg::records::mongo::replica_observation=debug".into()));
    let env: Vec<_> = routing
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let binary = Path::new(env!("CARGO_BIN_EXE_cmaeg"));
    trace.stage("setup", "end", 0);
    // A protocol floor persists after activation, so disabled controls use fresh databases.
    trace.stage("migration", "start", 0);
    let database = Arc::new(if ownership_enabled {
        DatabaseFixture::replicas(binary).await
    } else {
        DatabaseFixture::new(binary).await
    });
    let hello = database
        .admin
        .database("admin")
        .run_command(doc! {"hello": 1})
        .await
        .unwrap();
    assert!(
        hello.get_str("setName").is_ok(),
        "a replica set is required"
    );
    trace.stage("migration", "end", 0);
    let mut processes = Vec::new();
    for ordinal in 1..=replicas {
        trace.stage("ready", "start", ordinal);
        let ordinal_text = ordinal.to_string();
        let replicas_text = replicas.to_string();
        let mut env = env.clone();
        env.push(("CMAEG_HOST_REPLICAS", &replicas_text));
        env.push(("CMAEG_HOST_ORDINAL", &ordinal_text));
        processes.push(
            Process::with_providers(
                &mut fixture,
                providers.clone(),
                20000,
                &sink.base,
                &env,
                Some(database.clone()),
            )
            .await,
        );
        trace.stage("ready", "end", ordinal);
    }
    assert_eq!(
        processes
            .iter()
            .map(|process| process.child.id().unwrap())
            .collect::<HashSet<_>>()
            .len(),
        replicas,
        "each replica must be an independent operating-system process"
    );
    let envelope = Envelope::new();
    let identity = EventIdentity {
        owner: channel.definition.owner.subject.clone(),
        channel_id: fixture.id.clone(),
        record_id: channel.pointer.record_id.clone(),
        event_id: EventId::new(envelope.payload["message_id"].as_str().unwrap()).unwrap(),
    };
    let mut failures = workload::Workload {
        fixture: &fixture,
        processes: &mut processes,
        database: &database,
        identity: &identity,
        envelope: &envelope,
        enabled: ownership_enabled,
        trace,
        ledger: &mut ledger,
    }
    .run()
    .await;
    failures.extend(workload::stop(&mut processes, trace, &mut ledger).await);
    trace.stage("reconcile", "start", 0);
    ledger.record(json!({"stage":"channel_records_start", "records":null}));
    let records = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        database
            .admin
            .database(&database.name)
            .collection::<mongodb::bson::Document>("eg_channel_records")
            .count_documents(doc! {})
            .await
    })
    .await;
    let records = match records {
        Ok(Ok(count)) => Some(count),
        _ => None,
    };
    ledger.record(json!({"stage":"channel_records_end", "records":records}));
    if records != Some(1) {
        failures.push("channel_records_mismatch");
    }
    let provider_posts = provider
        .seen()
        .iter()
        .filter(|request| request.method == "POST")
        .count();
    let effects = sink.texts();
    ledger.record(json!({"stage":"effects", "provider_posts":provider_posts,
        "sink_effects":effects.len(), "failures":failures}));
    if provider_posts != 1 {
        failures.push("provider_admission_not_unique");
    }
    if effects.len() != 1 || effects.iter().any(|text| text != "hello") {
        failures.push("terminal_effect_not_unique");
    }
    trace.stage("reconcile", "end", 0);
    trace.stage("database_cleanup", "start", 0);
    drop(processes);
    let cleanup = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        // End only this disposable database's sessions after its children exit;
        // abandoned transactions otherwise outlive their client connection.
        database
            .admin
            .database("admin")
            .run_command(doc! {"killAllSessions": [
                {"user":"runtime", "db":&database.name},
                {"user":"migrator", "db":&database.name},
            ]})
            .await?;
        database.admin.database(&database.name).drop().await
    })
    .await;
    let cleanup_outcome = match &cleanup {
        Ok(Ok(())) => "complete",
        Ok(Err(_)) => "driver_error",
        Err(_) => "timeout",
    };
    ledger.record(json!({"stage":"database_cleanup", "outcome":cleanup_outcome, "complete":matches!(cleanup, Ok(Ok(())))}));
    if !matches!(cleanup, Ok(Ok(()))) {
        failures.push("database_cleanup_failed");
    }
    trace.stage("database_cleanup", "end", 0);
    ledger.record(json!({"stage":"final", "failures":failures, "complete":failures.is_empty()}));
    if failures.contains(&"cross_ingress_not_accepted") {
        panic!("the same admitted event must remain accepted through every ingress; failures={failures:?}");
    }
    assert!(failures.is_empty(), "host cohort failures: {failures:?}");
    eprintln!("replica_fixture replicas={replicas} status=PASS ownership={ownership_enabled}");
    trace.stage("teardown", "start", 0);
}
