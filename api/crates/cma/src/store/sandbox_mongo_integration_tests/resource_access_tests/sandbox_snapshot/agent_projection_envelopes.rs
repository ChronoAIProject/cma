use super::{batch_fixture::*, *};
use layer2_core::sandbox::HierarchyReadError;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn projected_agents_preserve_legacy_and_segmented_envelope_contracts() {
    let (client, name) = database("sandbox_projection_envelopes").await;
    let (store, target) = fixture(&client, &name).await;
    let record = template(&store, &target).await;
    let caller = Caller {
        subject: record.sandbox.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let expected = service(store.clone())
        .sandbox(&caller, &record.sandbox.id)
        .await
        .unwrap();
    let original = store.agents.find_one(doc! {}).await.unwrap().unwrap();
    for version in [
        Bson::Int32(1),
        Bson::Int64(1),
        Bson::Int32(2),
        Bson::Int64(2),
    ] {
        let mut valid = original.clone();
        let segmented = matches!(version, Bson::Int32(2) | Bson::Int64(2));
        valid.insert("document_schema_version", version);
        if segmented {
            valid.insert("agent_log_format", crate::store::agent_log_segments::FORMAT);
            valid.insert(
                "agent_log_incarnation",
                crate::store::agent_log_segments::incarnation(&record.agents[0]),
            );
            valid.insert("agent_log_head_revision", 1_i64);
        }
        store
            .agents
            .replace_one(doc! {}, valid.clone())
            .await
            .unwrap();
        for hierarchy_mode in 0..3 {
            for projected in [false, true] {
                let reader =
                    service(mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected));
                assert_eq!(
                    reader.sandbox(&caller, &record.sandbox.id).await.unwrap(),
                    expected
                );
            }
        }
        let changes = if segmented {
            vec![
                doc! {"$unset":{"agent_log_format":""}},
                doc! {"$unset":{"agent_log_incarnation":""}},
                doc! {"$unset":{"agent_log_head_revision":""}},
                doc! {"$set":{"agent_log_format":"unsupported"}},
                doc! {"$set":{"agent_log_incarnation":"0".repeat(64)}},
                doc! {"$set":{"agent_log_incarnation":"A".repeat(64)}},
                doc! {"$set":{"agent_log_head_revision":0_i64}},
                doc! {"$set":{"agent_log_head_revision":1_i32}},
                doc! {"$set":{"record.created_at":"changed-time"}},
            ]
        } else {
            vec![
                doc! {"$set":{"agent_log_format":"segmented_v1"}},
                doc! {"$set":{"agent_log_incarnation":"0".repeat(64)}},
                doc! {"$set":{"agent_log_head_revision":1_i64}},
            ]
        };
        for change in changes.into_iter().chain([
            doc! {"$set":{"creation_key":"wrong-query-key"}},
            doc! {"$unset":{"record.creation_key":""}},
            doc! {"$unset":{"record.created_at":""}},
            doc! {"$set":{"record.created_at":9}},
            doc! {"$set":{"document_schema_version":3}},
        ]) {
            store.agents.update_one(doc! {}, change).await.unwrap();
            for hierarchy_mode in 0..3 {
                for projected in [false, true] {
                    let reader = service(
                        mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected),
                    );
                    assert!(matches!(
                        reader.sandbox(&caller, &record.sandbox.id).await,
                        Err(HierarchyReadError::Corrupt(_))
                    ));
                }
            }
            store
                .agents
                .replace_one(doc! {}, valid.clone())
                .await
                .unwrap();
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn projected_detail_preserves_missing_roots_and_server_timeouts() {
    let (admin, name) = database("sandbox_projection_failures").await;
    let (store, target) = fixture(&admin, &name).await;
    let record = template(&store, &target).await;
    for hierarchy_mode in 0..3 {
        for projected in [false, true] {
            let reader = mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected);
            assert!(reader
                .read_sandbox_detail(&SandboxId("sbx_absent".into()))
                .await
                .unwrap()
                .is_none());
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":{"times":1},
                    "data":{"failCommands":["find"], "errorCode":50}
                })
                .await
                .unwrap();
            let result = reader.read_sandbox_detail(&record.sandbox.id).await;
            admin
                .database("admin")
                .run_command(doc! {
                    "configureFailPoint":"failCommand", "mode":"off"
                })
                .await
                .unwrap();
            assert!(matches!(result, Err(SandboxStoreError::Unavailable(_))));
            assert!(reader
                .read_sandbox_detail(&record.sandbox.id)
                .await
                .unwrap()
                .is_some());
        }
    }
    admin.database(&name).drop().await.unwrap();
}
