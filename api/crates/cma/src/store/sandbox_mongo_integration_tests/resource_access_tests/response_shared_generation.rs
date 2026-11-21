use super::*;
use crate::store::agent_log_segments::{self as segments, audit, mongo_tests::fixtures};

pub(super) async fn adopted() -> (Fixture, layer2_core::ids::AgentResponseId) {
    let (f, id) = retained(1_000).await;
    let db = f.client.database(&f.name);
    segments::schema::migrate(&db).await.unwrap();
    let entries = f.store.agent_log_prefix(&f.agent.id, 1_000).await.unwrap();
    f.store
        .agents
        .delete_one(doc! {"_id":f.agent.id.as_str()})
        .await
        .unwrap();
    f.store
        .agent_log
        .delete_many(doc! {"agent_id":f.agent.id.as_str()})
        .await
        .unwrap();
    fixtures::install(&f.store, f.agent.clone(), entries, 2, 400).await;
    for _ in 0..2 {
        assert_eq!(
            audit::run_unit(&f.client, &db, f.agent.id.as_str(), "shared-output", false)
                .await
                .unwrap(),
            "running"
        );
    }
    assert_eq!(
        audit::run_unit(&f.client, &db, f.agent.id.as_str(), "shared-output", false)
            .await
            .unwrap(),
        "complete"
    );
    audit::install(&f.client, &db, f.agent.id.as_str(), "shared-output")
        .await
        .unwrap();
    (f, id)
}

#[tokio::test]
#[ignore = "requires the disposable Mongo replica-set runner"]
async fn warm_output_cache_checks_independent_generation_projection_and_audit_state() {
    let (f, id) = adopted().await;
    let db = f.client.database(&f.name);
    let filter = doc! {"_id":f.agent.id.as_str()};
    let mut original_agent = f
        .store
        .agents
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let head = f
        .store
        .segment_logs
        .heads
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let projection = f
        .store
        .segment_logs
        .projections
        .find_one(filter.clone())
        .await
        .unwrap()
        .unwrap();
    let servers = servers_mode(&f, true).await;
    let client = client();
    let path = format!("/api/v2/responses/{id}");
    let response = request(&client, &f, &servers[0], &path)
        .send()
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        reqwest::StatusCode::OK,
        "{}",
        response.text().await.unwrap()
    );
    // Cold acquisition fences source deletion; warm reads must add no writes.
    assert!(!original_agent.contains_key("resource_share_guard"));
    original_agent.insert("resource_share_guard", 1_i64);
    assert_eq!(
        f.store
            .agents
            .find_one(filter.clone())
            .await
            .unwrap()
            .unwrap(),
        original_agent
    );
    let expected = request(&client, &f, &servers[1], &path)
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert!(expected.to_string().contains("retained answer"));

    // A consistent new head/projection generation cannot reuse old evidence,
    // even though no field in the Agent envelope or domain aggregate changes.
    for collection in [
        &f.store.segment_logs.heads,
        &f.store.segment_logs.projections,
    ] {
        collection
            .update_one(
                filter.clone(),
                doc! {"$set":{"evidence_generation":"absent-output-generation"}},
            )
            .await
            .unwrap();
    }
    let refused = request(&client, &f, &servers[0], &path)
        .send()
        .await
        .unwrap();
    assert!(
        refused.status().is_server_error(),
        "{}",
        refused.text().await.unwrap()
    );
    f.store
        .segment_logs
        .heads
        .replace_one(filter.clone(), head)
        .await
        .unwrap();
    f.store
        .segment_logs
        .projections
        .replace_one(filter.clone(), projection.clone())
        .await
        .unwrap();
    assert_eq!(
        request(&client, &f, &servers[1], &path)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        expected
    );

    f.store
        .segment_logs
        .projections
        .delete_one(filter.clone())
        .await
        .unwrap();
    assert!(request(&client, &f, &servers[0], &path)
        .send()
        .await
        .unwrap()
        .status()
        .is_server_error());
    f.store
        .segment_logs
        .projections
        .insert_one(projection)
        .await
        .unwrap();
    let audits = db.collection::<mongodb::bson::Document>(segments::AUDITS);
    let audit_filter = doc! {"agent_id":f.agent.id.as_str(),"audit_id":"shared-output"};
    audits
        .update_one(audit_filter.clone(), doc! {"$set":{"status":"corrupt"}})
        .await
        .unwrap();
    assert!(request(&client, &f, &servers[1], &path)
        .send()
        .await
        .unwrap()
        .status()
        .is_server_error());
    assert_eq!(
        f.store.agents.find_one(filter).await.unwrap().unwrap(),
        original_agent
    );
    audits
        .update_one(audit_filter, doc! {"$set":{"status":"installed"}})
        .await
        .unwrap();
    assert_eq!(
        request(&client, &f, &servers[0], &path)
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        expected
    );
    drop(servers);
    f.close().await;
}
