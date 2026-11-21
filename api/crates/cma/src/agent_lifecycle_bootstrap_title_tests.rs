use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn bootstrap_title_mode_survives_admission_and_recovery() {
    for mode in [None, Some("named"), Some("automatic")] {
        let mut body = json!({"name":"New task", "initial_message":"Compare tools"});
        if let Some(mode) = mode {
            body["title_mode"] = json!(mode);
        }
        let f = Fixture::with_request(request(body)).await;
        let task = start(f.service.clone(), f.owner.caller.clone(), work(&f.record));
        let (record, agent, _) = f.published().await;
        cancel(task).await;
        assert_eq!(
            agent.title.as_deref(),
            (mode != Some("automatic")).then_some("New task")
        );
        super::super::title_tests::assert_persisted_title(&f.client, &f.db, agent.clone(), mode)
            .await;
        f.db.collection::<Document>("agent_lifecycle_operations")
            .update_one(doc! {"_id": &record.id}, doc! {"$set": {"agent_id": null}})
            .await
            .unwrap();
        let interrupted = f.record().await;
        let restarted = f.restart().await;
        let replay = start(restarted, f.owner.caller.clone(), work(&interrupted));
        let (_, recovered, _) = f.published().await;
        assert_eq!(recovered.id, agent.id);
        assert_eq!(recovered.title, agent.title);
        assert_eq!(
            recovered.conversation.first_message,
            agent.conversation.first_message
        );
        cancel(replay).await;
        f.db.drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable local replica set"]
async fn explicit_new_task_rename_fences_a_late_generated_title() {
    use layer2_core::sandbox::store::AgentLogStore;
    use layer2_core::sandbox::{
        AgentLogKind, AgentTitle, AgentTitleClaimRequest, AgentTitleSource, AgentTitleStore,
    };
    let f = fixture::Fixture::with_request(request(json!({
        "name":"New task", "title_mode":"automatic", "initial_message":"Compare tools"
    })))
    .await;
    let task = start(
        f.service.clone(),
        f.owner.caller.clone(),
        fixture::work(&f.record),
    );
    let (_, mut agent, _) = f.published().await;
    cancel(task).await;
    agent.status = layer2_core::sandbox::AgentStatus::Ready;
    agent.conversation.last_seq = 0;
    let store = crate::store::MongoSandboxStore::open(&f.client, f.db.name())
        .await
        .unwrap();
    store.create_agent(agent.clone()).await.unwrap();
    for kind in [
        AgentLogKind::TurnStarted {
            model: None,
            turn_sequence: 1,
            origin_key: None,
        },
        AgentLogKind::OwnerMessage {
            content: "Compare tools".into(),
        },
        AgentLogKind::AgentMessage {
            content: "A comparison".into(),
        },
        AgentLogKind::TurnCompleted { turn_sequence: 1 },
    ] {
        store
            .append_agent_log(&agent.id, layer2_core::now_rfc3339(), kind)
            .await
            .unwrap();
    }
    let claim = store
        .acquire_agent_title_claim(
            &AgentTitleClaimRequest::new(agent.id.clone(), "late-title"),
            std::time::SystemTime::now(),
        )
        .await
        .unwrap()
        .unwrap();
    store
        .set_user_agent_title(
            &agent.id,
            &AgentTitle::new("New task").unwrap(),
            layer2_core::now_rfc3339(),
        )
        .await
        .unwrap();
    assert!(!store
        .complete_agent_title(
            &claim,
            &AgentTitle::new("Late generated title").unwrap(),
            AgentTitleSource::Generated,
            layer2_core::now_rfc3339()
        )
        .await
        .unwrap());
    let reopened = crate::store::MongoSandboxStore::open(&f.client, f.db.name())
        .await
        .unwrap();
    let title = reopened.agent_title(&agent.id).await.unwrap().unwrap();
    assert_eq!(title.title.as_str(), "New task");
    assert_eq!(title.source, AgentTitleSource::User);
    f.db.drop().await.unwrap();
}
