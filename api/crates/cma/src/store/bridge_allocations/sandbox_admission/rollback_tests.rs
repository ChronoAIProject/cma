use super::*;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_sandbox_late_policy_or_created_event_collision_rolls_back_actual_resource_insert() {
    for failure in ["policy", "event"] {
        let (client, database, bridges, svc, id, allocations) = setup().await;
        let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
        let request = request(&svc, &bridges, &id, &allocations, "sender").await;
        if failure == "policy" {
            resources
                .resource_policies
                .insert_one(doc! {"_id":request.sandbox().id.as_str()})
                .await
                .unwrap();
        } else {
            resources
                .sandbox_events
                .insert_one(
                    crate::store::sandbox_events::event_document(
                        &layer2_core::sandbox::SandboxEvent {
                            sandbox_id: request.sandbox().id.clone(),
                            seq: 1,
                            appended_at: request.sandbox().created_at.clone(),
                            kind: layer2_core::sandbox::SandboxEventKind::Created,
                        },
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }
        assert!(allocations
            .admit_channel_sandbox(request.clone())
            .await
            .is_err());
        assert!(resources
            .get_sandbox(&request.sandbox().id)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            resources
                .sandbox_admissions
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        let retained = allocations
            .generation(&request.previous().identity)
            .await
            .unwrap()
            .unwrap();
        assert!(!retained.effect_started);
        assert_eq!(retained.state, AllocationState::Reserved);
        assert_eq!(
            resources
                .resource_policies
                .count_documents(doc! {})
                .await
                .unwrap(),
            u64::from(failure == "policy")
        );
        assert_eq!(
            resources
                .sandbox_events
                .count_documents(doc! {})
                .await
                .unwrap(),
            u64::from(failure == "event")
        );
        client.database(&database).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_sandbox_owner_quota_includes_ordinary_admission_and_rolls_back_all_linkage() {
    let (client, database, bridges, svc, id, allocations) = setup().await;
    let resources = MongoSandboxStore::open(&client, &database).await.unwrap();
    let request = request(&svc, &bridges, &id, &allocations, "sender").await;
    for index in 0..3 {
        let mut ordinary = request.sandbox().clone();
        ordinary.id = layer2_core::SandboxId(format!("sbx_ordinary_{index}"));
        ordinary.name = format!("ordinary-{index}");
        resources.create_sandbox(ordinary).await.unwrap();
    }
    assert!(matches!(
        allocations.admit_channel_sandbox(request.clone()).await,
        Err(AllocationError::Capacity)
    ));
    assert!(resources
        .get_sandbox(&request.sandbox().id)
        .await
        .unwrap()
        .is_none());
    let retained = allocations
        .generation(&request.previous().identity)
        .await
        .unwrap()
        .unwrap();
    assert!(!retained.effect_started);
    assert_eq!(
        resources
            .sandbox_events
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        resources
            .resource_policies
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    resources
        .sandboxes
        .delete_one(doc! {"_id":"sbx_ordinary_0"})
        .await
        .unwrap();
    assert!(
        allocations
            .admit_channel_sandbox(request)
            .await
            .unwrap()
            .created
    );
    client.database(&database).drop().await.unwrap();
}
