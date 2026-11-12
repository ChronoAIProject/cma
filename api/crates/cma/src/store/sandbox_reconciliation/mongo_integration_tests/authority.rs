use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_authority_pages_every_owner_and_lifecycle_state_and_validates_rows() {
    let (client, name, store) = database().await;
    let states = [
        SandboxStatus::Provisioning,
        SandboxStatus::Installing,
        SandboxStatus::Active,
        SandboxStatus::Pausing,
        SandboxStatus::Paused,
        SandboxStatus::Reviving,
        SandboxStatus::CredentialRotating,
        SandboxStatus::Rotating,
        SandboxStatus::Failed,
        SandboxStatus::Retiring,
        SandboxStatus::Retired,
    ];
    for (index, status) in states.into_iter().enumerate().rev() {
        let mut value = active_sandbox(&format!("sbx_authority_{index:02}"));
        value.owner = SubjectId(format!("owner-{index}"));
        value.lifecycle.checkpoints.latest = Some(checkpoint(&[]));
        match status {
            SandboxStatus::Pausing => value.begin_pause(BEFORE.into(), None).unwrap(),
            SandboxStatus::Reviving => {
                value.status = SandboxStatus::Paused;
                value.runtime = None;
                value.work.schedule(SandboxWorkStep::None, BEFORE.into());
                value
                    .begin_revive(SandboxReviveCause::ExplicitRequest, BEFORE.into())
                    .unwrap();
            }
            SandboxStatus::Rotating => {
                let identity = SandboxRotationIdentity::capture(&value, &[]).unwrap();
                value
                    .begin_image_rotation("new-image".into(), identity, BEFORE.into())
                    .unwrap();
            }
            _ => value.status = status,
        }
        if status == SandboxStatus::Paused {
            value.runtime = None;
        }
        if status == SandboxStatus::Failed {
            value.failure = Some(TypedFailure {
                class: "runtime_lost".into(),
                detail: "gone".into(),
            });
        }
        if status == SandboxStatus::Retired {
            value.retired_at = Some(BEFORE.into());
        }
        store
            .sandboxes
            .insert_one(sandbox_document(&value, 0).unwrap())
            .await
            .unwrap();
    }
    let mut cursor = None;
    let mut found = Vec::new();
    loop {
        let page = store
            .sandbox_runtime_authority(&PageRequest::new(3, cursor).unwrap())
            .await
            .unwrap();
        found.extend(page.items);
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        found
            .iter()
            .map(|sandbox| sandbox.status)
            .collect::<Vec<_>>(),
        states
    );
    assert!(found.last().unwrap().runtime.is_some());
    for limit in [0, MAX_PAGE_SIZE + 1] {
        assert_eq!(
            store
                .sandbox_runtime_authority(&PageRequest {
                    limit,
                    cursor: None
                })
                .await,
            Err(SandboxStoreError::InvalidPage)
        );
    }
    store
        .sandboxes
        .update_one(
            doc! { "_id": found[0].id.as_str() },
            doc! { "$set": { "record.id": "sbx_different" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .sandbox_runtime_authority(&PageRequest::default())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}
