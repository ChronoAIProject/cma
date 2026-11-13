use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_abort_after_receipt_rolls_back_archive_custody_and_parent_fences() {
    let (client, database, store, _, controller, data) = fixture().await;
    let before = store
        .bindings
        .records
        .find_one(doc! {"_id":&data.binding.binding_id})
        .await
        .unwrap();
    let old_sandbox = store
        .sandboxes
        .find_one(doc! {"_id":data.sandbox.id.as_str()})
        .await
        .unwrap();
    let mut session = client.start_session().await.unwrap();
    start_transaction(&mut session).await.unwrap();
    let tentative = controller
        .apply_preparation(&data.intent, &data.key, &mut session)
        .await
        .unwrap();
    session.abort_transaction().await.unwrap();
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .bindings
            .records
            .find_one(doc! {"_id":&data.binding.binding_id})
            .await
            .unwrap(),
        before
    );
    assert_eq!(
        store
            .sandboxes
            .find_one(doc! {"_id":data.sandbox.id.as_str()})
            .await
            .unwrap(),
        old_sandbox
    );
    assert!(controller
        .custody
        .get(&tentative.reserved.sandbox_id, 1)
        .await
        .unwrap()
        .is_none());
    let committed = controller
        .prepare_retired_binding(&data.intent, &data.key)
        .await
        .unwrap();
    assert_ne!(committed.reserved.sandbox_id, tentative.reserved.sandbox_id);
    assert_eq!(
        store
            .bindings
            .preparations
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    client.database(&database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_refuses_busy_foreign_and_corrupt_children_without_partial_writes() {
    for change in [
        "pending",
        "owner",
        "generation",
        "index",
        "selection_index",
        "command_selection_index",
        "sandbox",
        "workspace_cleanup",
    ] {
        let (client, database, store, _, controller, data) = fixture().await;
        let mut ready = data.binding.clone();
        ready.state = BindingState::Ready;
        let mut conversation = BindingConversation::new(
            &ready,
            "retained_conversation".into(),
            super::super::super::mongo_tests::NOW.into(),
        )
        .unwrap();
        conversation.state = layer2_core::sandbox::binding::BindingConversationState::Retired;
        match change {
            "pending" => {
                conversation.state =
                    layer2_core::sandbox::binding::BindingConversationState::Retiring
            }
            "owner" => conversation.owner = layer2_core::SubjectId::new("other"),
            "generation" => conversation.generation += 1,
            "sandbox" => conversation.sandbox_id = layer2_core::SandboxId::generate(),
            _ => {}
        }
        let mut document = conversation_document(&conversation).unwrap();
        if change == "index" {
            document.insert("generation", 1_i64);
        }
        if change == "selection_index" {
            document.insert("binding_id", "unrelated-binding");
        }
        store
            .bindings
            .conversations
            .insert_one(document)
            .await
            .unwrap();
        if change == "command_selection_index" {
            // Even an absent/deleted Agent cannot hide retained queued work
            // by a malformed command identity/status lookup projection.
            store.agent_commands.insert_one(doc! {
                "_id":"stale-command", "document_schema_version":DOCUMENT_SCHEMA_VERSION,
                "agent_id":"unrelated-agent", "status":"completed",
                "record":{"receipt":{"envelope":{"agent_id":conversation.agent_id.as_str()}}, "status":"queued"},
            }).await.unwrap();
        }
        if change == "workspace_cleanup" {
            store
                .workspaces
                .update_one(
                    doc! {"_id":data.workspace.id.as_str()},
                    doc! {"$set":{"owned_workspace_cleanup_admission_v1":{"test":true}}},
                )
                .await
                .unwrap();
        }
        let before_binding = store
            .bindings
            .records
            .find_one(doc! {"_id":&data.binding.binding_id})
            .await
            .unwrap();
        let before_sandbox = store
            .sandboxes
            .find_one(doc! {"_id":data.sandbox.id.as_str()})
            .await
            .unwrap();
        let before_workspace = store
            .workspaces
            .find_one(doc! {"_id":data.workspace.id.as_str()})
            .await
            .unwrap();
        assert!(
            controller
                .prepare_retired_binding(&data.intent, &data.key)
                .await
                .is_err(),
            "accepted {change}"
        );
        assert_eq!(
            store
                .bindings
                .preparations
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .bindings
                .records
                .find_one(doc! {"_id":&data.binding.binding_id})
                .await
                .unwrap(),
            before_binding
        );
        assert_eq!(
            store
                .sandboxes
                .find_one(doc! {"_id":data.sandbox.id.as_str()})
                .await
                .unwrap(),
            before_sandbox
        );
        assert_eq!(
            store
                .workspaces
                .find_one(doc! {"_id":data.workspace.id.as_str()})
                .await
                .unwrap(),
            before_workspace
        );
        client.database(&database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_preparation_and_binding_retirement_have_one_exact_revision_winner() {
    let (client, database, store, _, controller, data) = fixture().await;
    let mut retired = data.binding.clone();
    retired.state = BindingState::Retired;
    retired
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    let (preparation, retirement) = tokio::join!(
        controller.prepare_retired_binding(&data.intent, &data.key),
        store.save_binding(Some(data.binding.revision), retired.clone())
    );
    let current = store
        .binding(&data.binding.binding_id)
        .await
        .unwrap()
        .unwrap();
    match (preparation, retirement) {
        (Ok(receipt), Err(_)) => {
            assert_eq!(current, receipt.reserved);
            assert_eq!(
                store
                    .bindings
                    .preparations
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                1
            );
        }
        (Err(_), Ok(())) => {
            assert_eq!(current, retired);
            assert_eq!(
                store
                    .bindings
                    .preparations
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0
            );
            assert!(controller
                .prepare_retired_binding(&data.intent, &data.key)
                .await
                .is_err());
        }
        (a, b) => panic!("reservation and retirement did not serialize: {a:?}, {b:?}"),
    }
    client.database(&database).drop().await.unwrap();
}
