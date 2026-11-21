use super::{batch_fixture::*, *};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn projected_agents_avoid_envelope_fallback_but_preserve_workspace_fallback() {
    let (admin, name) = database("sandbox_projection_envelope_size").await;
    let trace = Arc::new(Trace::default());
    let client = observed_client(&trace).await;
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
    for (collection, is_agent) in [(&store.agents, true), (&store.workspaces, false)] {
        let original = collection.find_one(doc! {}).await.unwrap().unwrap();
        let mut padded = original.clone();
        padded.insert("retained_padding", "");
        let padding = 16 * 1024 * 1024 - 64 - padded.to_vec().unwrap().len();
        padded.insert("retained_padding", "x".repeat(padding));
        // Install retained fixtures without a large old/new replacement write.
        collection.delete_one(doc! {}).await.unwrap();
        collection.insert_one(padded).await.unwrap();
        for projected in [false, true] {
            clear(&trace);
            let reader = service(mode(&store, 2).with_sandbox_agent_projection(projected));
            assert_eq!(
                reader.sandbox(&caller, &record.sandbox.id).await.unwrap(),
                expected
            );
            let commands = trace.commands.lock().unwrap().clone();
            let agent_finds = commands
                .iter()
                .filter(|(_, command)| command.get_str("find").ok() == Some("agents"))
                .count();
            assert_eq!(agent_finds, usize::from(!is_agent || !projected));
            let hierarchy: Vec<_> = commands
                .iter()
                .filter(|(_, command)| {
                    command.get_str("find").ok() == Some("sandboxes")
                        || command.get_str("find").ok() == Some("workspaces")
                        || command.get_str("find").ok() == Some("worktrees")
                        || command.get_str("find").ok() == Some("agents")
                        || command.get_str("aggregate").ok() == Some("workspaces")
                })
                .collect();
            assert_eq!(hierarchy.len(), if is_agent && projected { 2 } else { 5 });
            let time = hierarchy[1]
                .1
                .get_document("readConcern")
                .unwrap()
                .get_timestamp("atClusterTime")
                .unwrap();
            for (_, command) in &hierarchy[1..] {
                assert_eq!(
                    command
                        .get_document("readConcern")
                        .unwrap()
                        .get_timestamp("atClusterTime")
                        .unwrap(),
                    time
                );
                assert_eq!(
                    command.get_document("lsid").unwrap(),
                    hierarchy[0].1.get_document("lsid").unwrap()
                );
            }
        }
        collection.delete_one(doc! {}).await.unwrap();
        collection.insert_one(original).await.unwrap();
    }
    admin.database(&name).drop().await.unwrap();
    client.shutdown().await;
    admin.shutdown().await;
}
