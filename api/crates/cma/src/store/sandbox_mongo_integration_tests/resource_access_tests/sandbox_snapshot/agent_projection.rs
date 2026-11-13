use super::{access_measurement_trace as meter, batch_fixture::*, *};
use layer2_core::sandbox::HierarchyReadError;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn projected_agents_keep_detail_parity_without_unused_payload_bytes() {
    let (admin, name) = database("sandbox_agent_projection_bytes").await;
    let trace = Arc::new(meter::Meter::default());
    let client = meter::client(&trace).await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    let root = seed(&store, &template, "projected", 4, 4).await;
    let caller = Caller {
        subject: root.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let expected = service(store.clone())
        .sandbox(&caller, &root.id)
        .await
        .unwrap();
    // A retained optional payload increases full BSON size without changing the
    // decoded Agent. Projection size must be independent of that payload.
    let mut unpadded_bytes = [0; 3];
    for padded in [false, true] {
        if padded {
            store
                .agents
                .update_many(
                    doc! {"sandbox_id": root.id.as_str()},
                    doc! {"$set": {"retained_padding": "x".repeat(256 * 1024)}},
                )
                .await
                .unwrap();
        }
        for hierarchy_mode in 0..3 {
            let mut bytes = [0; 2];
            for projected in [false, true] {
                trace.clear();
                let reader =
                    service(mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected));
                assert_eq!(reader.sandbox(&caller, &root.id).await.unwrap(), expected);
                let stats = trace.stats.lock().unwrap().clone();
                assert_eq!(stats.agent_docs, 16);
                assert!(stats.commands.keys().all(|key| !key.contains("agent_log")));
                bytes[usize::from(projected)] = stats.agent_bytes;
                assert_eq!(stats.count(), [11, 10, 8][hierarchy_mode]);
                println!("PROJECTION_BYTES mode={hierarchy_mode} projected={projected} padded={padded} agents={} bytes={} commands={}", stats.agent_docs, stats.agent_bytes, stats.count());
            }
            if padded {
                assert_eq!(bytes[1], unpadded_bytes[hierarchy_mode]);
            } else {
                unpadded_bytes[hierarchy_mode] = bytes[1];
            }
            assert!(bytes[1] < bytes[0]);
            assert!(
                bytes[1] < 16 * 1024,
                "lineage payload stays bounded independently of padding"
            );
        }
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn projection_omits_conversation_checks_only_for_sandbox_details() {
    let (client, name) = database("sandbox_projection_validation_boundary").await;
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
    store
        .agents
        .update_one(
            doc! {},
            doc! {"$set": {"record.conversation.active_turn":1}},
        )
        .await
        .unwrap();
    for hierarchy_mode in 0..3 {
        for projected in [false, true] {
            let configured = mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected);
            let reader = service(configured.clone());
            let detail = reader.sandbox(&caller, &record.sandbox.id).await;
            if projected {
                assert_eq!(detail.unwrap(), expected);
            } else {
                assert!(matches!(detail, Err(HierarchyReadError::Corrupt(_))));
            }
            assert!(matches!(
                reader.workspace(&caller, &record.workspaces[0].id).await,
                Err(HierarchyReadError::Corrupt(_))
            ));
            assert_eq!(
                configured.resource_access_snapshot(&target).await,
                Err(ResourceAccessStoreError::Corrupt)
            );
            // Lifecycle and retirement callers keep the complete aggregate port.
            let full = configured
                .read_sandbox_hierarchy(&record.sandbox.id)
                .await
                .unwrap()
                .unwrap();
            assert!(full.agents[0].validate_conversation().is_err());
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn projection_preserves_all_lineage_corruption_refusals() {
    let (client, name) = database("sandbox_projection_corruption").await;
    let (store, target) = fixture(&client, &name).await;
    let record = template(&store, &target).await;
    let caller = Caller {
        subject: record.sandbox.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let original = store.agents.find_one(doc! {}).await.unwrap().unwrap();
    for change in [
        doc! {"owner": "incorrect-query-owner"},
        doc! {"workspace_id": "incorrect-query-workspace"},
        doc! {"worktree_id": "incorrect-query-worktree"},
        doc! {"record.id": "incorrect-aggregate-id"},
        doc! {"revision": 9},
        doc! {"revision": 0, "record.revision": 0},
        doc! {"owner": "foreign-owner", "record.owner": "foreign-owner"},
        doc! {"worktree_id": "missing-worktree", "record.worktree_id": "missing-worktree"},
        doc! {"workspace_id": "missing-workspace", "record.workspace_id": "missing-workspace"},
        doc! {"document_schema_version": 999},
        doc! {"record.revision": "invalid-type"},
    ] {
        store
            .agents
            .update_one(doc! {}, doc! {"$set": change})
            .await
            .unwrap();
        for hierarchy_mode in 0..3 {
            for projected in [false, true] {
                let reader =
                    service(mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected));
                assert!(matches!(
                    reader.sandbox(&caller, &record.sandbox.id).await,
                    Err(HierarchyReadError::Corrupt(_))
                ));
            }
        }
        store
            .agents
            .replace_one(doc! {}, original.clone())
            .await
            .unwrap();
    }
    store.agents.delete_one(doc! {}).await.unwrap();
    for hierarchy_mode in 0..3 {
        for projected in [false, true] {
            let reader =
                service(mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected));
            assert!(matches!(
                reader.sandbox(&caller, &record.sandbox.id).await,
                Err(HierarchyReadError::Corrupt(_))
            ));
        }
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn projected_detail_keeps_empty_hierarchies_and_concealment() {
    let (client, name) = database("sandbox_projection_empty").await;
    let (store, target) = fixture(&client, &name).await;
    let template = template(&store, &target).await;
    let caller = Caller {
        subject: template.sandbox.owner.clone(),
        authorization: Authorization::new([Capability::UseOwnSandboxes]),
    };
    let foreign = Caller {
        subject: SubjectId::new("foreign-fixture"),
        authorization: caller.authorization.clone(),
    };
    for (label, workspaces, agents) in [("empty", 0, 0), ("workspaces", 4, 0), ("one", 1, 1)] {
        let root = seed(&store, &template, label, workspaces, agents).await;
        let expected = service(store.clone())
            .sandbox(&caller, &root.id)
            .await
            .unwrap();
        for hierarchy_mode in 0..3 {
            for projected in [false, true] {
                let reader =
                    service(mode(&store, hierarchy_mode).with_sandbox_agent_projection(projected));
                assert_eq!(reader.sandbox(&caller, &root.id).await.unwrap(), expected);
                assert_eq!(
                    reader.sandbox(&foreign, &root.id).await,
                    Err(HierarchyReadError::NotFound)
                );
                assert_eq!(
                    reader
                        .sandbox(&caller, &SandboxId("sbx_absent".into()))
                        .await,
                    Err(HierarchyReadError::NotFound)
                );
            }
        }
    }
    client.database(&name).drop().await.unwrap();
}
