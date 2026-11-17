use super::*;

fn integer(value: &mongodb::bson::Bson) -> i64 {
    match value {
        mongodb::bson::Bson::Int32(number) => i64::from(*number),
        mongodb::bson::Bson::Int64(number) => *number,
        other => panic!("expected an integer: {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn summary_lookup_uses_existing_indexes_and_bounds_revision_history() {
    let (client, name, store, meter) = measured_store().await;
    for kind in KINDS {
        let (profiles, revisions) = collections(&store, kind);
        for depth in [1, 64] {
            profiles.delete_many(doc! {}).await.unwrap();
            revisions.delete_many(doc! {}).await.unwrap();
            seed(&store, kind, 201, depth, false).await;
            let pipeline = crate::store::profiles::summaries::pipeline(
                revisions.name(),
                &SubjectId("owner".into()),
                &ProfilePageRequest {
                    limit: 200,
                    cursor: None,
                },
            );
            let explain = client
                .database(&name)
                .run_command(doc! {
                    "explain": {"aggregate": profiles.name(), "pipeline": pipeline,
                        "cursor": {}}, "verbosity": "executionStats",
                })
                .await
                .unwrap();
            let stages = explain.get_array("stages").unwrap();
            let root = stages[0]
                .as_document()
                .unwrap()
                .get_document("$cursor")
                .unwrap();
            let stats = root.get_document("executionStats").unwrap();
            assert_eq!(integer(&stats["totalDocsExamined"]), 201);
            let plan = root
                .get_document("queryPlanner")
                .unwrap()
                .get_document("winningPlan")
                .unwrap()
                .to_string();
            assert!(
                plan.contains("_id_") || plan.contains("owner_updated"),
                "{plan}"
            );
            let lookup = stages
                .iter()
                .filter_map(|stage| stage.as_document())
                .find(|stage| stage.contains_key("$lookup"))
                .unwrap();
            assert_eq!(integer(&lookup["collectionScans"]), 0);
            assert_eq!(lookup.get_array("indexesUsed").unwrap().len(), 1);
            assert!(lookup["indexesUsed"]
                .to_string()
                .contains("profile_revision"));
            assert_eq!(
                integer(&lookup["totalDocsExamined"]),
                201 * i64::from(depth.min(2))
            );
            assert!(integer(&lookup["totalKeysExamined"]) <= 402);
            meter.start();
            let (status, page) = get(
                &routes(store.clone()),
                "owner",
                &format!("/api/v1/{kind}-profiles?limit=200"),
            )
            .await;
            let counts = meter.finish();
            assert_eq!(status, StatusCode::OK);
            assert_eq!(page["items"].as_array().unwrap().len(), 200);
            assert!(counts.commands <= 3);
            println!("profile-summary-index kind={kind} revisions={depth} root_docs=201 revision_docs={} commands={}", integer(&lookup["totalDocsExamined"]), counts.commands);
        }
    }
    client.database(&name).drop().await.unwrap();
}
