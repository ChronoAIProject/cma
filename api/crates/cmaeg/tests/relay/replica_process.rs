//! Independent executables sharing one authenticated replica-set database.
//! Provider fixture admission proves this exact fixture contract, not native sends.
#[path = "../support/replica_delivery.rs"]
mod delivery;
#[allow(dead_code)] // Reuse the existing full HTTP snapshot/SSE fixture in this integration binary.
#[path = "../../src/turns/routed/adapter_test_server.rs"]
mod replay_provider;

use crate::{
    fake_nyxid_reply::FakeReply,
    runner_process::{database::DatabaseFixture, Process},
    support::{Envelope, Fixture},
};
use cmaeg::ingress::routing_store::EventIdentity;
use cmaeg_core::ids::EventId;
use delivery::{routed_channel, routed_env, runner_keys, DeliveryOracle};
use mongodb::bson::doc;
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path, sync::Arc, time::Duration};

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
    for replicas in [1, 2, 3] {
        verify_replicas(replicas, false).await;
        verify_replicas(replicas, true).await;
    }
}

async fn verify_replicas(replicas: usize, ownership_enabled: bool) {
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
    let env: Vec<_> = routing
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let binary = Path::new(env!("CARGO_BIN_EXE_cmaeg"));
    // A protocol floor persists after activation, so disabled controls use fresh databases.
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
    let mut processes = Vec::new();
    for _ in 0..replicas {
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
    let (body, token) = envelope.signed();
    let requests = processes.iter().map(|process| {
        let mut request = fixture.request(body.clone(), &token).build().unwrap();
        *request.url_mut() = format!("{}/v1/callbacks/{}", process.base, fixture.id)
            .parse()
            .unwrap();
        fixture.client.execute(request)
    });
    let oracle = DeliveryOracle::open(&database.uri, &database.name, runner_keys()).await;
    let mut rejected = None;
    let mut accepted = 0;
    for response in futures_util::future::join_all(requests).await {
        let response = response.unwrap();
        let status = response.status();
        let value: Value = response.json().await.unwrap();
        let reason = ingress_reason(&value);
        if status != 202 {
            rejected = Some(status);
            eprintln!("replica_rejection replicas={replicas} ownership={ownership_enabled} ingress_rejection={reason}");
        } else {
            assert_eq!(value["status"].as_str(), Some("accepted"));
            accepted += 1;
        }
    }
    if let Some(status) = rejected {
        // This is an after-response observation, not proof of the failed request's queue commit.
        let plan = oracle.plan(&identity).await;
        let target_present = if let Some(plan) = &plan {
            oracle
                .targets(plan)
                .await
                .values()
                .filter(|target| target.is_some())
                .count()
        } else {
            0
        };
        eprintln!("replica_failure replicas={replicas} ownership={ownership_enabled} accepted={accepted} durable_plan={} target_present={target_present}", plan.is_some());
        assert_eq!(
                status,
                202,
            "the same admitted event must remain accepted through every ingress; replicas={replicas}"
            );
    }
    let plan = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(plan) = oracle.plan(&identity).await {
                let targets = oracle.targets(&plan).await;
                if !targets.is_empty()
                    && targets.values().all(|value| {
                        value
                            .as_ref()
                            .is_some_and(|value| value.confirmed_terminal_text())
                    })
                {
                    break plan;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("durable confirmed delivery deadline");
    assert_eq!(plan.targets.len(), 1);
    for process in &mut processes {
        process.terminate().await;
        assert!(
            tokio::time::timeout(Duration::from_secs(20), process.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        process.assert_safe_logs();
    }
    assert_eq!(
        provider
            .seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1,
        "one provider command identity survives concurrent ingress"
    );
    // Independent sink observations detect a duplicate hidden by journal settlement.
    assert_eq!(
        sink.texts().len(),
        1,
        "one complete terminal fixture effect"
    );
    assert!(sink.texts().iter().all(|text| text == "hello"));
    assert_eq!(
        database
            .admin
            .database(&database.name)
            .collection::<mongodb::bson::Document>("eg_channel_records")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    drop(processes);
    database
        .admin
        .database(&database.name)
        .drop()
        .await
        .unwrap();
    eprintln!("replica_fixture replicas={replicas} status=PASS ownership={ownership_enabled}");
}
