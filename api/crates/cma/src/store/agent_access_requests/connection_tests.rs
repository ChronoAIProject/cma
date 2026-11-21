use super::*;
use layer2_core::ids::AgentCommandId;
use layer2_core::sandbox::store::SandboxStore;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn mongo_access_connections_remain_available_during_applying_without_granting_authority() {
    let (client, name, store, first) = super::super::grant::tests::paused_fixture().await;
    let before = store
        .get_sandbox(&first.requester.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let services = vec![ServiceId::new("calendar"), ServiceId::new("github")];
    let mut record = first.clone();
    if let Some(AgentAccessDecision::Approved {
        services: selected, ..
    }) = &mut record.decision
    {
        *selected = services.clone();
    }
    record.progress.grant_generation_floor = Some(0);
    record.progress.revive_operation_id = Some("saved-revival-operation".into());
    record.progress.delivered_credential = before.credential.current.clone();
    record.progress.delivered_runtime = Some(first.requester.runtime.clone());
    record.progress.notification_command_id = Some(AgentCommandId("access-notification".into()));
    for phase in [
        AgentAccessPhase::Connecting,
        AgentAccessPhase::WaitingQuiet,
        AgentAccessPhase::Pausing,
        AgentAccessPhase::Granting,
        AgentAccessPhase::Reviving,
        AgentAccessPhase::Notifying,
    ] {
        record.phase = phase;
        store
            .agent_access_requests
            .replace_one(doc! {"_id":&record.id}, document(&record).unwrap())
            .await
            .unwrap();
        assert_eq!(
            store
                .access_request_connection_services(&record.requester.owner, &record.id)
                .await
                .unwrap(),
            services,
            "phase {phase:?}"
        );
        assert_eq!(
            store
                .access_request_connection_services(&SubjectId::new("foreign"), &record.id)
                .await,
            Err(AgentAccessError::NotFound)
        );
        assert_eq!(
            store.read_access_request(&record.id).await.unwrap(),
            Some(record.clone())
        );
        assert_eq!(
            store
                .get_sandbox(&record.requester.sandbox_id)
                .await
                .unwrap(),
            Some(before.clone())
        );
    }
    for phase in [
        AgentAccessPhase::Pending,
        AgentAccessPhase::Denied,
        AgentAccessPhase::Failed,
        AgentAccessPhase::Succeeded,
    ] {
        let mut unavailable = record.clone();
        unavailable.phase = phase;
        unavailable.lease = None;
        if phase == AgentAccessPhase::Pending {
            unavailable.decision = None;
            unavailable.approved_at = None;
            unavailable.progress = AgentAccessProgress::default();
        } else if phase == AgentAccessPhase::Denied {
            unavailable.decision = Some(AgentAccessDecision::Denied {
                command_key_hash: "d".repeat(64),
            });
            unavailable.approved_at = None;
        }
        store
            .agent_access_requests
            .replace_one(doc! {"_id":&record.id}, document(&unavailable).unwrap())
            .await
            .unwrap();
        assert_eq!(
            store
                .access_request_connection_services(&record.requester.owner, &record.id)
                .await,
            Err(AgentAccessError::Conflict),
            "phase {phase:?}"
        );
    }
    store
        .agent_access_requests
        .replace_one(doc! {"_id":&record.id}, document(&record).unwrap())
        .await
        .unwrap();
    store
        .agents
        .update_one(
            doc! {"_id":record.requester.agent_id.as_str()},
            doc! {"$set":{"record.conversation.conversation_id":"replacement-native"}},
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .access_request_connection_services(&record.requester.owner, &record.id)
            .await,
        Err(AgentAccessError::NotFound)
    );
    assert_eq!(
        store
            .get_sandbox(&record.requester.sandbox_id)
            .await
            .unwrap(),
        Some(before)
    );
    client.database(&name).drop().await.unwrap();
}
