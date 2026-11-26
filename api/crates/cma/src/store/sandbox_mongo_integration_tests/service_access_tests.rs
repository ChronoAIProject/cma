use super::*;
use layer2_core::sandbox::{SandboxCheckpointRef, SandboxServiceGrant};
use layer2_core::ServiceId;
use layer3_adapter::{CheckpointReceipt, WriterState};

fn paused() -> Sandbox {
    let mut value = sandbox("sbx_service_access");
    value.status = SandboxStatus::Paused;
    value.work = SandboxWorkState::new(SandboxWorkStep::None, NOW.into());
    value.credential.reaches = vec![ServiceId::new("chrono-llm"), ServiceId::new("ornn-api")];
    value.lifecycle.checkpoints.latest = Some(
        SandboxCheckpointRef::seal(
            &CheckpointReceipt {
                epoch: 1,
                object_reference: "checkpoints/access".into(),
                sha256: "a".repeat(64),
                writers: WriterState::Resumed,
            },
            &[],
            NOW,
        )
        .unwrap(),
    );
    value
}

fn transaction(before: &Sandbox, next: Sandbox, grant: SandboxServiceGrant) -> SandboxTransaction {
    SandboxTransaction::SaveSandboxAndAppend {
        command_revival: None,
        released_runtime: None,
        expected_revision: before.revision,
        sandbox: Box::new(next),
        event_at: MINUTE.into(),
        event: SandboxEventKind::ServiceAccessGranted { grant },
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn service_access_is_atomic_restart_durable_and_refuses_implicit_scope_changes() {
    let (client, name) = database("service_access").await;
    let store = MongoSandboxStore::open(&client, &name).await.unwrap();
    let before = paused();
    store.create_sandbox(before.clone()).await.unwrap();
    let mut changed = before.clone();
    let grant = changed
        .grant_service_access(
            &before.owner,
            &[ServiceId::new("api-github")],
            MINUTE.into(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        store
            .save_sandbox(before.revision, changed.clone())
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    for index in 0..4 {
        let mut invalid = changed.clone();
        let mut forged = grant.clone();
        match index {
            0 => invalid.profile.revision += 1,
            1 => forged.authorized_by = SubjectId::new("other-owner"),
            2 => invalid.credential.reaches.push(ServiceId::new("implicit")),
            _ => forged.services.push(ServiceId::new("extra")),
        }
        assert_eq!(
            store
                .commit(transaction(&before, invalid, forged))
                .await
                .unwrap_err(),
            SandboxStoreError::Conflict
        );
        assert_eq!(
            store.get_sandbox(&before.id).await.unwrap(),
            Some(before.clone())
        );
        assert!(store
            .sandbox_events(&before.id, 0, 20)
            .await
            .unwrap()
            .is_empty());
    }
    store
        .commit(transaction(&before, changed.clone(), grant.clone()))
        .await
        .unwrap();
    drop(store);
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    assert_eq!(
        reopened.get_sandbox(&before.id).await.unwrap(),
        Some(changed.clone())
    );
    let events = reopened.sandbox_events(&before.id, 0, 20).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].kind,
        SandboxEventKind::ServiceAccessGranted { grant }
    );
    let mut removed = changed.clone();
    removed.credential.service_grants.clear();
    removed.revision += 1;
    assert_eq!(
        reopened
            .save_sandbox(changed.revision, removed)
            .await
            .unwrap_err(),
        SandboxStoreError::Conflict
    );
    let mut preloaded = changed.clone();
    preloaded.id = SandboxId("sbx_preloaded".into());
    preloaded.revision = 1;
    assert!(reopened.create_sandbox(preloaded).await.is_err());
    client.database(&name).drop().await.unwrap();
}
