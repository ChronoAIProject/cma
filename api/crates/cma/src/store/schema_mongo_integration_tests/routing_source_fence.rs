use layer2_core::sandbox::{store::SandboxStore, RuntimeBinding};
use mongodb::bson::{doc, Document};

use super::database;
use crate::store::{
    runtime_routing::MongoRuntimeRouting,
    sandbox_mongo::{
        commit_transaction, is_transient, start_transaction, CommitOutcome, MongoSandboxStore,
    },
    sandbox_mongo_integration_tests::sandbox,
    sandboxes::read_sandbox,
    transaction_terminal::GuardedTerminal as _,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable real MongoDB replica set"]
async fn routing_source_write_fence_conflicts_with_generation_adoption() {
    for fence_source in [false, true] {
        let (client, name) = database().await;
        let db = client.database(&name);
        let store = MongoSandboxStore::open(&client, &name).await.unwrap();
        MongoRuntimeRouting::open(&db, "fixture", false)
            .await
            .unwrap();
        let routes = db.collection::<Document>("runtime_backend_routes");
        let mut original = sandbox("sbx_routing_adoption");
        let binding = RuntimeBinding {
            boot_id: None,
            runtime_id: "agent-sandbox:fixture:retained".into(),
            runtime_version: "fixture-image".into(),
            provision_generation: None,
            bound_at: original.created_at.clone(),
        };
        original.runtime = Some(binding.clone());
        store.create_sandbox(original.clone()).await.unwrap();

        let mut session = client.start_session().await.unwrap();
        start_transaction(&mut session).await.unwrap();
        let row = store
            .sandboxes
            .find_one(doc! {"_id":original.id.as_str()})
            .session(&mut session)
            .await
            .unwrap()
            .unwrap();
        let observed = read_sandbox(&row).unwrap();
        let generation = observed.lifecycle.runtime_generation_high_water.max(1);
        assert_eq!(generation, 1);
        let id =
            layer3_adapter::provider_request_digest(&("fixture", observed.id.as_str(), generation))
                .unwrap();

        let mut successor = original.clone();
        successor
            .record_adopted_runtime_generation(&binding, 2, "2026-09-05T00:01:00Z".into())
            .unwrap();
        store
            .save_sandbox(original.revision, successor.clone())
            .await
            .unwrap();
        assert_eq!(
            store.get_sandbox(&original.id).await.unwrap(),
            Some(successor)
        );

        if fence_source {
            let error = store
                .sandboxes
                .update_one(
                    doc! {"_id":observed.id.as_str(),"revision":row.get_i64("revision").unwrap()},
                    doc! {"$inc":{"runtime_routing_progress_guard":1_i64}},
                )
                .session(&mut session)
                .await
                .unwrap_err();
            assert!(is_transient(&error), "{error}");
            let _ = session.abort_guarded().await;
        } else {
            routes
                .insert_one(doc! {
                    "_id": &id, "document_schema_version": 1_i64,
                    "deployment": "fixture", "sandbox_id": observed.id.as_str(),
                    "generation": 1_i64, "backend": "agent-sandbox"
                })
                .session(&mut session)
                .await
                .unwrap();
            assert!(matches!(
                commit_transaction(&mut session).await,
                CommitOutcome::Committed
            ));
        }

        assert_eq!(
            routes.find_one(doc! {"_id":&id}).await.unwrap().is_some(),
            !fence_source
        );
        db.drop().await.unwrap();
    }
}
