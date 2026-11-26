use super::*;
use crate::store::{
    agent_access_tools::tests::{persisted, publish},
    runtime_routing::MongoRuntimeRouting,
    sandbox_mongo::record_bson,
    sandbox_mongo_integration_tests::sandbox,
    sandboxes::sandbox_document,
};
use layer2_core::sandbox::store::AgentStore;
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
        cursor: Some("retained-first".into()),
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
async fn online_backfill_endpoints_require_the_confirmed_scope_and_revision() {
    let (client, database, store, snapshot) = persisted().await;
    let db = client.database(&database);
    let source = snapshot.creation.unwrap();
    publish(&store, &source.agent_id).await;
    let agent = store.get_agent(&source.agent_id).await.unwrap().unwrap();
    let origin = "https://target.example.test";
    let cohort = Cohort::Endpoints(crate::config::AgentAccessMigration {
        revision: 2,
        origin: origin.into(),
        agents: vec![agent.id.to_string()],
    });
    let opened = run_id();
    start(&db, &opened, &cohort).await.unwrap();
    assert_eq!(
        step(&db, &opened, &cohort, 0).await.unwrap()["counts"]["pending"],
        1
    );
    assert_eq!(
        step(&db, &opened, &cohort, 1).await.unwrap()["complete"],
        false
    );

    let runtime = snapshot.resource.lineage.sandbox.runtime.unwrap();
    db.collection::<Document>("agent_access_tools")
        .insert_one(doc! {
            "_id": "fixture-receipt",
            "document_schema_version": 1_i64,
            "agent_id": agent.id.as_str(),
            "record": {
                "transport_origin": format!("{origin}/"),
                "conversation": "original-native",
                "scope": {"creation": record_bson(&source).unwrap(), "runtime": record_bson(&runtime).unwrap()},
            },
        })
        .await
        .unwrap();
    let before_publication = run_id();
    start(&db, &before_publication, &cohort).await.unwrap();
    assert_eq!(
        step(&db, &before_publication, &cohort, 0).await.unwrap()["counts"]["pending"],
        1
    );
    let agents = db.collection::<Document>("agents");
    agents
        .update_one(
            doc! {"_id": agent.id.as_str()},
            doc! {"$set": {"record.conversation.managed_guidance_revision": 2_i64}},
        )
        .await
        .unwrap();
    let confirmed = run_id();
    start(&db, &confirmed, &cohort).await.unwrap();
    assert_eq!(
        step(&db, &confirmed, &cohort, 0).await.unwrap()["counts"]["changed"],
        1
    );
    assert_eq!(
        step(&db, &confirmed, &cohort, 1).await.unwrap()["complete"],
        true
    );

    let different_target = Cohort::Endpoints(crate::config::AgentAccessMigration {
        revision: 2,
        origin: "https://different.example.test".into(),
        agents: vec![agent.id.to_string()],
    });
    assert!(step(&db, &confirmed, &different_target, 2).await.is_err());
    db.drop().await.unwrap();
}
