use super::*;
use crate::store::{
    agent_access_tools::{
        tests::{key, persisted, publish, request},
        AccessRecordAuthority, MongoAgentAccessToolSource,
    },
    runtime_routing::MongoRuntimeRouting,
    sandbox_mongo::record_bson,
    sandbox_mongo_integration_tests::sandbox,
    sandboxes::sandbox_document,
};
use layer2_core::sandbox::{
    store::{AgentProvenanceStore, AgentStore},
    AgentCleanupDebt, AgentOperationIntent, AgentOperationRecord, AgentStatus, AgentWorkLease,
    AgentWorkReceipt, AgentWorkStep,
};
use layer3_adapter::{
    hosted::HostedAccessToolSource, AgentOpenRequest, AgentProcessAction, AgentRestartRuntime,
    RuntimeHandle, RuntimeStatusRequest,
};
use mongodb::Client;

fn run_id() -> String {
    Uuid::new_v4().to_string()
}

fn routing() -> Cohort {
    Cohort::Routing {
        deployment: "fixture".into(),
        allocate_native: false,
    }
}

async fn database() -> Database {
    let client = Client::with_uri_str(
        std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable replica set required"),
    )
    .await
    .unwrap();
    client.database(&format!("cma_backfill_{}", Uuid::new_v4().simple()))
}

#[test]
fn online_backfill_reconciles_counts_and_redacts_checkpoint_keys() {
    let counts = {
        let mut counts = Counts::default();
        for observation in [
            Observation::Changed,
            Observation::Retained,
            Observation::Skipped,
            Observation::Pending,
            Observation::Error,
        ] {
            counts.observe(observation).unwrap();
        }
        counts
    };
    assert_eq!(counts.scanned, 5);
    assert!(counts.valid());
    assert!(!Counts {
        scanned: 6,
        ..counts.clone()
    }
    .valid());
    let scan = Scan {
        run: run_id(),
        document_schema_version: 1,
        cohort: "fixture".into(),
        scope_digest: "a".repeat(64),
        started_at: DateTime::now(),
        revision: 1,
        snapshot_time: Timestamp {
            time: 1,
            increment: 1,
        },
        cursor: Some(doc! {"_id": "retained-first"}),
        counts,
        unobserved: 0,
        finished_at: None,
        routing_initialized: None,
        completion_proved: false,
    };
    let output = scan.public_view().to_string();
    assert!(!output.contains("retained-first"));
    assert!(!output.contains("retained-last"));
    assert!(decode(scan.document().unwrap()).is_ok());
    assert!(decode(doc! {"_id": scan.run, "document_schema_version": 2}).is_err());
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn online_backfill_routing_recovers_checkpoint_and_preserves_each_route() {
    let db = database().await;
    let cohort = routing();
    let sandboxes = db.collection::<Document>("sandboxes");
    let count = PAGE_SIZE + 1;
    let documents = (0..count)
        .map(|index| sandbox_document(&sandbox(&format!("sbx_fixture_{index:03}")), 0).unwrap());
    sandboxes.insert_many(documents).await.unwrap();
    MongoRuntimeRouting::open(&db, "fixture", false)
        .await
        .unwrap();

    let run = run_id();
    assert_eq!(start(&db, &run, &cohort).await.unwrap()["revision"], 0);
    let mut interrupted = db.client().start_session().await.unwrap();
    start_transaction(&mut interrupted).await.unwrap();
    assert_eq!(
        advance(&db, &run, &cohort, 0, &mut interrupted)
            .await
            .unwrap()
            .counts
            .scanned,
        PAGE_SIZE
    );
    interrupted.abort_transaction().await.unwrap();
    assert_eq!(current(&db, &run).await.unwrap().counts.scanned, 0);

    let (first, second) = tokio::join!(step(&db, &run, &cohort, 0), step(&db, &run, &cohort, 0),);
    assert!(first.is_ok() || second.is_ok());
    assert_eq!(step(&db, &run, &cohort, 0).await.unwrap()["revision"], 1);
    assert_eq!(current(&db, &run).await.unwrap().counts.scanned, PAGE_SIZE);

    // A deletion ahead of the cursor and an insertion behind it must not
    // change which rows the resumed run is proving.
    assert_eq!(
        sandboxes
            .delete_one(doc! {"_id": format!("sbx_fixture_{PAGE_SIZE:03}")})
            .await
            .unwrap()
            .deleted_count,
        1
    );
    sandboxes
        .insert_one(sandbox_document(&sandbox("sbx_fixture_000a"), 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        step(&db, &run, &cohort, 1).await.unwrap()["counts"]["retained"],
        count
    );

    let receipt = step(&db, &run, &cohort, 2).await.unwrap();
    assert_eq!(receipt["complete"], true);
    assert_eq!(receipt["routing_initialized"], true);
    assert_eq!(receipt["counts"]["scanned"], count);
    assert_eq!(receipt["counts"]["errors"], 0);
    assert_eq!(step(&db, &run, &cohort, 2).await.unwrap(), receipt);
    assert_eq!(start(&db, &run, &cohort).await.unwrap(), receipt);
    assert!(current(&db, &run).await.unwrap().cursor.is_none());
    assert_eq!(
        db.collection::<Document>("runtime_backend_routes")
            .count_documents(doc! {"backend": "opensandbox"})
            .await
            .unwrap(),
        count as u64
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn online_backfill_routing_requires_initialization_in_the_same_snapshot() {
    let db = database().await;
    let cohort = routing();
    db.collection::<Document>("sandboxes")
        .insert_one(sandbox_document(&sandbox("sbx_fixture_001"), 0).unwrap())
        .await
        .unwrap();

    let old_run = run_id();
    start(&db, &old_run, &cohort).await.unwrap();
    MongoRuntimeRouting::open(&db, "fixture", false)
        .await
        .unwrap();
    assert_eq!(
        step(&db, &old_run, &cohort, 0).await.unwrap()["counts"]["pending"],
        1
    );
    let prior = step(&db, &old_run, &cohort, 1).await.unwrap();
    assert_eq!(prior["complete"], false);
    assert_eq!(prior["routing_initialized"], false);
    assert!(!prior["finished_at"].is_null());

    let new_run = run_id();
    start(&db, &new_run, &cohort).await.unwrap();
    step(&db, &new_run, &cohort, 0).await.unwrap();
    let confirmed = step(&db, &new_run, &cohort, 1).await.unwrap();
    assert_eq!(confirmed["complete"], true);
    assert_eq!(confirmed["counts"]["retained"], 1);
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn online_backfill_accounts_for_malformed_members_across_pages() {
    let db = database().await;
    let cohort = routing();
    let sandboxes = db.collection::<Document>("sandboxes");
    let documents = (0..PAGE_SIZE)
        .map(|index| sandbox_document(&sandbox(&format!("sbx_fixture_{index:03}")), 0).unwrap());
    sandboxes.insert_many(documents).await.unwrap();
    MongoRuntimeRouting::open(&db, "fixture", false)
        .await
        .unwrap();
    let malformed = [
        doc! {"_id": ""},
        doc! {"_id": mongodb::bson::oid::ObjectId::new()},
    ];
    sandboxes.insert_many(malformed.clone()).await.unwrap();
    db.collection::<Document>("agents")
        .insert_many(malformed)
        .await
        .unwrap();

    let wildcard = endpoint_cohort(db.name(), vec!["*".into()]);
    for (cohort, scanned, finish_revision) in [(&cohort, PAGE_SIZE + 2, 2), (&wildcard, 2, 1)] {
        let run = run_id();
        start(&db, &run, cohort).await.unwrap();
        for revision in 0..finish_revision {
            step(&db, &run, cohort, revision).await.unwrap();
        }
        let receipt = step(&db, &run, cohort, finish_revision).await.unwrap();
        assert_eq!(receipt["counts"]["scanned"], scanned);
        assert_eq!(receipt["counts"]["errors"], 2);
        assert_eq!(receipt["complete"], false);
        assert!(!receipt["finished_at"].is_null());
    }

    db.drop().await.unwrap();
}

const OLD_ORIGIN: &str = "https://original.example.test";
const TARGET_ORIGIN: &str = "https://target.example.test";

fn endpoint_cohort(database: &str, agents: Vec<String>) -> Cohort {
    Cohort::Endpoints {
        migration: crate::config::AgentAccessMigration {
            revision: 2,
            origin: TARGET_ORIGIN.into(),
            agents,
        },
        authority: AccessRecordAuthority::new(
            database,
            TARGET_ORIGIN,
            &[OLD_ORIGIN.into()],
            &key(9),
        )
        .unwrap(),
    }
}

async fn endpoint_receipt(database: &Database, cohort: &Cohort) -> Value {
    let run = run_id();
    start(database, &run, cohort).await.unwrap();
    step(database, &run, cohort, 0).await.unwrap();
    step(database, &run, cohort, 1).await.unwrap()
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn online_backfill_endpoints_require_an_acknowledged_transport() {
    let (client, database, store, snapshot) = persisted().await;
    let db = client.database(&database);
    let creation = snapshot.creation.as_ref().unwrap();
    let cohort = endpoint_cohort(&database, vec![creation.agent_id.to_string()]);
    let runtime = RuntimeHandle("access-runtime".into());
    let original = MongoAgentAccessToolSource::new(store.clone(), OLD_ORIGIN, &key(9)).unwrap();
    let access = MongoAgentAccessToolSource::with_retained_origins(
        store.clone(),
        TARGET_ORIGIN,
        &[OLD_ORIGIN.into()],
        &key(9),
    )
    .unwrap();
    let open = request(&snapshot);
    let binding = original.resolve(&runtime, &open, 1).await.unwrap().unwrap();
    original
        .confirm_open(&binding, "original-native")
        .await
        .unwrap();
    publish(&store, &creation.agent_id).await;

    let mut agent = store.get_agent(&creation.agent_id).await.unwrap().unwrap();
    assert_eq!(agent.work.next_fence, 2);
    let retained = access
        .resolve(&runtime, &open, agent.work.next_fence)
        .await
        .unwrap()
        .unwrap();
    let mut settings = agent.settings();
    settings.model = "model-a".into();
    agent.conversation.settings = Some(settings.clone());
    let agents = db.collection::<Document>("agents");
    agents.update_one(
        doc! {"_id": agent.id.as_str()},
        doc! {"$set": {"record.conversation.settings": record_bson(&agent.conversation.settings).unwrap()}},
    ).await.unwrap();
    agent.conversation.operation = Some(
        AgentOperationRecord::new(
            1,
            1,
            1,
            AgentOperationIntent::RefreshContext {
                service_origin: Some(TARGET_ORIGIN.into()),
                revision: 2,
                readiness_receipt: "a".repeat(64),
                guidance: "Managed guidance".into(),
                previous_guidance: vec![],
                settings,
                working_directory: None,
            },
            layer2_core::now_rfc3339(),
        )
        .unwrap(),
    );
    agent.conversation.last_seq = 1;
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();

    let prepared = access
        .prepare_transport(&retained, TARGET_ORIGIN, "original-native", 1)
        .await
        .unwrap();
    assert_eq!(endpoint_receipt(&db, &cohort).await["counts"]["pending"], 1);
    access
        .confirm_transport(&prepared, "original-native", 1)
        .await
        .unwrap();
    assert_eq!(endpoint_receipt(&db, &cohort).await["counts"]["pending"], 1);
    agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"record.conversation.managed_guidance_revision": 2_i64}},
        )
        .await
        .unwrap();
    let receipt = endpoint_receipt(&db, &cohort).await;
    assert_eq!(receipt["counts"]["changed"], 1);
    assert_eq!(receipt["complete"], true);
    verify_restarted_endpoint(&db, &cohort, &store, &access, &runtime, &agent.id).await;

    let access_rows = db.collection::<Document>("agent_access_tools");
    let accepted = access_rows
        .find_one(doc! {"agent_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    let id = accepted.get_str("_id").unwrap();
    access_rows.delete_one(doc! {"_id": id}).await.unwrap();
    let mut partial = accepted;
    partial
        .get_document_mut("record")
        .unwrap()
        .remove("request");
    partial.get_document_mut("record").unwrap().remove("fence");
    let original_id = partial.insert("_id", "fixture-foreign-receipt").unwrap();
    access_rows.insert_one(partial.clone()).await.unwrap();
    let foreign = endpoint_receipt(&db, &cohort).await;
    assert_eq!(foreign["counts"]["pending"], 1);
    assert_eq!(foreign["complete"], false);

    partial.insert("_id", original_id);
    access_rows.insert_one(partial).await.unwrap();
    let refused = endpoint_receipt(&db, &cohort).await;
    assert_eq!(refused["counts"]["errors"], 1);
    assert_eq!(refused["complete"], false);
    db.drop().await.unwrap();
}

async fn verify_restarted_endpoint(
    database: &Database,
    cohort: &Cohort,
    store: &crate::store::MongoSandboxStore,
    access: &MongoAgentAccessToolSource,
    runtime: &RuntimeHandle,
    agent_id: &layer2_core::AgentId,
) {
    let snapshot = store.agent_provenance_snapshot(agent_id).await.unwrap();
    let lineage = &snapshot.resource.lineage;
    let sandbox = &lineage.sandbox;
    let pin = sandbox.runtime.as_ref().unwrap();
    let mut agent = lineage.agent.clone().unwrap();
    let tree = lineage.worktree.as_ref().unwrap();
    let mut open = AgentOpenRequest::new(
        AgentProcessAction::Open.operation_id(agent.id.as_str(), 2),
        crate::sandbox::runtime_options(&agent, sandbox, tree),
        agent.conversation.conversation_id.clone(),
    );
    open.restart_runtime = Some(AgentRestartRuntime {
        boot_id: pin.boot_id.clone().unwrap(),
        runtime: RuntimeStatusRequest {
            sandbox_id: sandbox.id.to_string(),
            generation: pin.provision_generation.unwrap(),
            home_identity: sandbox
                .lifecycle
                .home
                .as_ref()
                .unwrap()
                .attachment()
                .unwrap()
                .identity()
                .unwrap(),
        },
    });
    let fence = agent.work.next_fence;
    agent.status = AgentStatus::Starting;
    agent.work.step = AgentWorkStep::RestartConversation;
    agent.work.next_fence += 1;
    agent.work.lease = Some(AgentWorkLease {
        holder: "restart-fixture".into(),
        fence,
        expires_at: layer2_core::rfc3339_at(
            std::time::SystemTime::now() + std::time::Duration::from_secs(120),
        ),
    });
    agent
        .work
        .receipts
        .retain(|receipt| receipt.step != AgentWorkStep::OpenConversation);
    agent.work.cleanup_debt = vec![AgentCleanupDebt {
        step: AgentWorkStep::OpenConversation,
        operation_id: open.operation_id.clone(),
        request_sha256: open.request_sha256.clone(),
    }];
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    let enrolled = access
        .resolve(runtime, &open, fence)
        .await
        .unwrap()
        .unwrap();
    access
        .confirm_open(&enrolled, "original-native")
        .await
        .unwrap();

    agent.status = AgentStatus::Ready;
    agent.work.step = AgentWorkStep::None;
    agent.work.lease = None;
    agent.work.cleanup_debt.clear();
    agent.revision += 1;
    store
        .save_agent(agent.revision - 1, agent.clone())
        .await
        .unwrap();
    let agents = database.collection::<Document>("agents");
    agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"record.conversation.runtime_generation": 2_i64}},
        )
        .await
        .unwrap();
    let missing = endpoint_receipt(database, cohort).await;
    assert_eq!(missing["counts"]["pending"], 1);
    assert_eq!(missing["complete"], false);

    agent = store.get_agent(agent_id).await.unwrap().unwrap();
    agent.work.receipts.push(AgentWorkReceipt {
        step: AgentWorkStep::OpenConversation,
        operation_id: open.operation_id,
        request_sha256: open.request_sha256,
        completed_at: layer2_core::now_rfc3339(),
    });
    agent.revision += 1;
    store.save_agent(agent.revision - 1, agent).await.unwrap();
    let confirmed = endpoint_receipt(database, cohort).await;
    assert_eq!(confirmed["counts"]["changed"], 1);
    assert_eq!(confirmed["complete"], true);
}
