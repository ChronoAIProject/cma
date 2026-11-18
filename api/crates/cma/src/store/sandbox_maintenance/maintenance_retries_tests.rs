use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn maintenance_retries_transient_writes_and_reconciles_ambiguous_commits_once() {
    let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI")
        .expect("CHRONO_TEST_MONGODB_FAILPOINT_URI must name a failpoint-enabled replica set");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let app_name = format!("cma-maintenance-{suffix}");
    let name = format!("cma_maintenance_failpoint_{suffix}");
    let admin = Client::with_uri_str(&uri).await.unwrap();
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    options.app_name = Some(app_name.clone());
    let client = Client::with_options(options).unwrap();
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (_, policy) = policy_tests::enable(&client, &name).await;
    let mut global_fences = 0_i64;
    for scenario in [
        "transient",
        "ambiguous_pause",
        "ambiguous_rotation",
        "transient_global_pause",
        "ambiguous_global_pause",
    ] {
        let (sandbox, agent) = fixture(&store, &format!("sbx_{scenario}")).await;
        let (times, data) = if scenario.starts_with("transient") {
            (
                1_i32,
                doc! {
                    "failCommands": ["update"], "appName": &app_name,
                    "errorCode": 112_i32, "errorLabels": ["TransientTransactionError"],
                },
            )
        } else {
            (
                5_i32,
                doc! {
                    "failCommands": ["commitTransaction"], "appName": &app_name,
                    "writeConcernError": {
                        "code": 50_i32, "codeName": "MaxTimeMSExpired",
                        "errmsg": "injected ambiguous maintenance commit",
                    },
                },
            )
        };
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": { "times": times }, "data": data,
            })
            .await
            .unwrap();
        let result = if scenario == "ambiguous_rotation" {
            store.schedule_image_rotation(rotation(&sandbox.id)).await
        } else if scenario.ends_with("global_pause") {
            store
                .schedule_inactivity_pause(SandboxInactivityPauseRequest {
                    auto_pause_enabled: true,
                    expected_policy_revision: Some(policy.revision),
                    ..pause(&sandbox.id)
                })
                .await
        } else {
            store.schedule_inactivity_pause(pause(&sandbox.id)).await
        };
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint": "failCommand", "mode": "off",
            })
            .await
            .expect("disable failpoint before assertions");
        assert_eq!(result.unwrap(), SandboxMaintenanceOutcome::Scheduled);
        assert_eq!(
            store
                .sandbox_events(&sandbox.id, 0, 10)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .get_sandbox(&sandbox.id)
                .await
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        let row = store
            .agents
            .find_one(doc! { "_id": agent.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get_i64("sandbox_maintenance_fence").unwrap(), 1);
        if scenario.ends_with("global_pause") {
            global_fences += 1;
            let row = store
                .budget_settings
                .find_one(doc! {"_id":"platform"})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                row.get_i64("inactivity_admission_fence").unwrap(),
                global_fences
            );
        }
    }
    client.database(&name).drop().await.unwrap();
}
