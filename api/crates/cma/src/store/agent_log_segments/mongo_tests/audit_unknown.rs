use super::*;
use mongodb::options::ClientOptions;
use segments::audit::{self, test_support as hooks};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a disposable replica set"]
async fn committed_unknown_promotion_resumes_its_durable_cursor_without_rescanning_source() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let application = format!("audit-unknown-{suffix}");
    let mut options = ClientOptions::parse(uri).await.unwrap();
    options.app_name = Some(application.clone());
    let client = Client::with_options(options).unwrap();
    let name = format!("cma_segments_unknown_{suffix}");
    let db = client.database(&name);
    schema::migrate(&db).await.unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (agent, _) = super::audit_tests::conversation(&store, 180, 1).await;
    let claim = hooks::claim(&client, &db, agent.id.as_str(), "unknown")
        .await
        .unwrap();
    let validated = hooks::collect(&client, &db, claim).await.unwrap();
    admin.database("admin").run_command(doc! {"configureFailPoint":"failCommand","mode":{"times":5_i32},
        "data":{"failCommands":["commitTransaction"],"appName":application,
        "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired","errmsg":"injected audit commit ambiguity"}}}).await.unwrap();
    let outcome = hooks::promote(&client, &db, validated.clone()).await;
    let disabled = admin
        .database("admin")
        .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
        .await;
    disabled.expect("disable the isolated commit failpoint before assertions");
    assert!(matches!(outcome, Err(SandboxStoreError::Unavailable(_))));
    crate::store::test_visibility::majority_barrier(&admin, &name).await;
    let audits = db.collection::<Document>(AUDITS);
    let filter = doc! {"agent_id":agent.id.as_str(),"audit_id":"unknown"};
    let durable = audits.find_one(filter.clone()).await.unwrap().unwrap();
    assert!(durable.get_i64("step").unwrap() > validated.get_i64("step").unwrap());
    assert!(durable
        .get_document("stage")
        .unwrap()
        .get_str("cursor")
        .is_ok());
    assert_eq!(
        db.collection::<Document>(EVIDENCE)
            .count_documents(
                doc! {"evidence_generation":durable.get_str("result_generation").unwrap()}
            )
            .await
            .unwrap(),
        128
    );
    audits
        .update_one(filter, doc! {"$set":{"expires_at_ms":0_i64}})
        .await
        .unwrap();
    crate::store::mongo_read_telemetry::isolated_test(async {
        assert_eq!(
            audit::run_unit(&client, &db, agent.id.as_str(), "unknown", false)
                .await
                .unwrap(),
            "complete"
        );
        assert_eq!(
            crate::store::mongo_read_telemetry::snapshot("agent_log"),
            [0; 4]
        );
    })
    .await;
    assert_eq!(
        db.collection::<Document>(EVIDENCE)
            .count_documents(
                doc! {"evidence_generation":durable.get_str("result_generation").unwrap()}
            )
            .await
            .unwrap(),
        363 // original semantic evidence plus immutable output changes and turn totals
    );
    for (kind, expected) in [
        ("message", 181),
        ("turn", 1),
        ("output_change", 180),
        ("turn_output", 1),
    ] {
        assert_eq!(
            db.collection::<Document>(EVIDENCE)
                .count_documents(doc! {
                    "evidence_generation":durable.get_str("result_generation").unwrap(),"kind":kind
                })
                .await
                .unwrap(),
            expected,
            "{kind}"
        );
    }
    assert_eq!(
        audit::install(&client, &db, agent.id.as_str(), "unknown")
            .await
            .unwrap(),
        "installed"
    );
    db.drop().await.unwrap();
}
