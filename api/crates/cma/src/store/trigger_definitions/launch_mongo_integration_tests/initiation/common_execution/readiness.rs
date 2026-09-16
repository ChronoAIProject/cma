use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_readiness_requires_complete_receipts_and_open_original_resources() {
    let accepted = Accepted::new().await;
    let runtime = Runtime::new(false, false);
    let service = AgentOperationService::new(accepted.f.resources.clone(), runtime.clone());
    let database = accepted.f.seed.client.database(&accepted.f.seed.database);
    let original = accepted.current_command().await;
    service.trigger_ready_for_claim(&original).await.unwrap();
    for (name, id, closed_field) in [
        (
            "trigger_first_message_bindings",
            accepted.launch.id.as_str(),
            None,
        ),
        (
            "trigger_launch_input_protocols",
            accepted.launch.id.as_str(),
            None,
        ),
        ("trigger_launches", accepted.launch.id.as_str(), None),
        (
            "agents",
            accepted.agent.id.as_str(),
            Some("channel_reuse_cleanup_admission"),
        ),
        (
            "sandboxes",
            accepted.agent.sandbox_id.as_str(),
            Some("channel_reuse_cleanup_execution"),
        ),
        (
            "workspaces",
            accepted.agent.workspace_id.as_str(),
            Some("owned_workspace_cleanup_admission_v1"),
        ),
    ] {
        let collection = database.collection::<Document>(name);
        let before = collection.find_one(doc! {"_id":id}).await.unwrap().unwrap();
        if let Some(field) = closed_field {
            collection
                .update_one(doc! {"_id":id}, doc! {"$set":{field:{"closed":true}}})
                .await
                .unwrap();
        } else {
            collection.delete_one(doc! {"_id":id}).await.unwrap();
        }
        assert!(
            service.trigger_ready_for_claim(&original).await.is_err(),
            "{name}"
        );
        assert_eq!(runtime.sends.load(Ordering::SeqCst), 0);
        let command = accepted
            .f
            .resources
            .get_command(&original.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            command, original,
            "readiness mutated the command after {name} refusal"
        );
        collection
            .replace_one(doc! {"_id":id}, before)
            .upsert(true)
            .await
            .unwrap();
        service.trigger_ready_for_claim(&original).await.unwrap();
    }
    let claimed = accepted.claim().await;
    assert!(service.trigger_ready_for_claim(&original).await.is_err());
    assert!(service.trigger_ready_for_claim(&claimed).await.is_err());
    service.execute_trigger_claim(&claimed).await.unwrap();
    assert_eq!(runtime.sends.load(Ordering::SeqCst), 1);
    assert!(service
        .trigger_ready_for_claim(&accepted.current_command().await)
        .await
        .is_err());
    accepted.f.close().await;
}
