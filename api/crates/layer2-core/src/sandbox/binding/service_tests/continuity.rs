use super::*;

#[tokio::test]
async fn unsupported_updates_and_reconciliation_retries_preserve_original_work() {
    for field in ["services", "skills", "prompt", "environment", "affinity"] {
        let f = fixture();
        let binding = f.ready().await;
        let (conversation, _) = f
            .service
            .ensure_conversation(&f.caller, "binding-one", "conversation_one")
            .await
            .unwrap();
        f.service
            .reconcile_conversation(&binding, &conversation)
            .await
            .unwrap();
        let agent = f
            .store
            .get_agent(&conversation.agent_id)
            .await
            .unwrap()
            .unwrap();
        let worktree = f
            .store
            .get_worktree(&conversation.worktree_id)
            .await
            .unwrap();
        let sandbox = f.store.get_sandbox(&binding.sandbox_id).await.unwrap();
        let workspace = f.store.get_workspace(&binding.workspace_id).await.unwrap();
        let retained = f
            .store
            .binding_conversation("binding-one", 1, "conversation_one")
            .await
            .unwrap();
        let mut changed = profile("Original instruction");
        match field {
            "services" => changed["permission"]["service_scope"] = json!(["extra-service"]),
            "skills" => {
                changed["context"]["skills"] =
                    json!([{ "source":"ornn", "id":"new-skill", "version":"1.0" }])
            }
            "prompt" => changed["context"]["system_prompt"] = json!("Revised instruction"),
            "affinity" => changed["environment"]["session_affinity"] = json!("source_user"),
            _ => changed["environment"]["size"]["cpu_cores"] = json!(2),
        }
        let digest = format!("sha256:{}", content_digest(&changed).unwrap());
        for _ in 0..3 {
            assert!(
                matches!(
                    f.service
                        .put(&f.caller, "binding-one", changed.clone(), &digest, &f.key)
                        .await,
                    Err(BindingError::ReconfigurationUnsupported)
                ),
                "{field}"
            );
            let observed = f.binding().await;
            assert_eq!(observed, binding);
            f.service.reconcile_binding(&observed).await.unwrap();
            f.service
                .reconcile_conversation(&observed, retained.as_ref().unwrap())
                .await
                .unwrap();
            assert_eq!(
                f.store.get_agent(&conversation.agent_id).await.unwrap(),
                Some(agent.clone())
            );
            assert_eq!(
                f.store
                    .get_worktree(&conversation.worktree_id)
                    .await
                    .unwrap(),
                worktree
            );
            assert_eq!(
                f.store.get_sandbox(&binding.sandbox_id).await.unwrap(),
                sandbox
            );
            assert_eq!(
                f.store.get_workspace(&binding.workspace_id).await.unwrap(),
                workspace
            );
            assert_eq!(
                f.store
                    .binding_conversation("binding-one", 1, "conversation_one")
                    .await
                    .unwrap(),
                retained
            );
            assert!(f.binding().await.previous.is_none());
        }
        // An unchanged retry remains supported after the refused update.
        assert_eq!(f.put("binding-one", "Original instruction").await, binding);
        let (replayed, created) = f
            .service
            .ensure_conversation(&f.caller, "binding-one", "conversation_one")
            .await
            .unwrap();
        assert!(!created);
        assert_eq!(replayed.agent_id, agent.id);
        assert_eq!(replayed.worktree_id, agent.worktree_id);
    }
}

#[tokio::test]
async fn metadata_update_keeps_the_existing_conversation_and_can_be_replayed() {
    let f = fixture();
    let original = f.ready().await;
    let (conversation, _) = f
        .service
        .ensure_conversation(&f.caller, "binding-one", "conversation_one")
        .await
        .unwrap();
    f.service
        .reconcile_conversation(&original, &conversation)
        .await
        .unwrap();
    let agent = f.store.get_agent(&conversation.agent_id).await.unwrap();
    let mut value = profile("Original instruction");
    value["metadata"]["name"] = json!("Renamed binding");
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    let next = f
        .service
        .put(&f.caller, "binding-one", value.clone(), &digest, &f.key)
        .await
        .unwrap();
    assert_eq!(next.generation, original.generation);
    assert_eq!(next.sandbox_id, original.sandbox_id);
    assert_eq!(next.workspace_id, original.workspace_id);
    assert_eq!(next.work, BindingWork::Idle);
    assert!(next.previous.is_none());
    f.service.reconcile_binding(&next).await.unwrap();
    f.service
        .reconcile_conversation(&next, &conversation)
        .await
        .unwrap();
    assert_eq!(
        f.store.get_agent(&conversation.agent_id).await.unwrap(),
        agent
    );
    assert_eq!(
        f.service
            .put(&f.caller, "binding-one", value, &digest, &f.key)
            .await
            .unwrap(),
        next
    );
}
