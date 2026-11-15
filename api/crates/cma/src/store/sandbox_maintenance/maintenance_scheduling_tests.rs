use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_pages_globally_in_exclusive_id_order_and_rejects_corrupt_rows() {
    let (client, name, store) = database().await;
    for (id, owner, status) in [
        ("sbx_z", "owner-a", SandboxStatus::Active),
        ("sbx_a", "owner-b", SandboxStatus::Active),
        ("sbx_m", "owner-a", SandboxStatus::Active),
        ("sbx_b", "owner-c", SandboxStatus::Provisioning),
    ] {
        let mut value = active_sandbox(id);
        value.owner = SubjectId(owner.into());
        value.status = status;
        store.create_sandbox(value).await.unwrap();
    }
    let first = store
        .sandbox_maintenance_candidates(&PageRequest::new(2, None).unwrap())
        .await
        .unwrap();
    assert_eq!(
        first.items,
        [SandboxId("sbx_a".into()), SandboxId("sbx_m".into())]
    );
    assert_eq!(first.next.as_deref(), Some("sbx_m"));
    let second = store
        .sandbox_maintenance_candidates(&PageRequest::new(2, first.next).unwrap())
        .await
        .unwrap();
    assert_eq!(second.items, [SandboxId("sbx_z".into())]);
    assert_eq!(second.next, None);
    for limit in [0, MAX_PAGE_SIZE + 1] {
        assert_eq!(
            store
                .sandbox_maintenance_candidates(&PageRequest {
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
            doc! { "_id": "sbx_a" },
            doc! { "$set": { "record.id": "sbx_other" } },
        )
        .await
        .unwrap();
    assert!(matches!(
        store
            .sandbox_maintenance_candidates(&PageRequest::default())
            .await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_pause_reads_every_live_agent_and_preserves_model_rows() {
    let (client, name, store) = database().await;
    let (sandbox, first) = fixture(&store, "sbx_idle").await;
    let second = ready_agent(&sandbox, "agt_second");
    insert_agent(&store, &second, LATER).await;
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(
        store.get_sandbox(&sandbox.id).await.unwrap(),
        Some(sandbox.clone())
    );
    store
        .agent_log
        .update_one(
            doc! { "agent_id": second.id.as_str(), "seq": 1_i64 },
            doc! { "$set": { "record.appended_at": IDLE_AT } },
        )
        .await
        .unwrap();
    store
        .append_sandbox_event(&sandbox.id, IDLE_AT.into(), SandboxEventKind::Created)
        .await
        .unwrap();
    assert_eq!(
        store
            .schedule_inactivity_pause(pause(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let paused = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    assert_eq!(paused.status, SandboxStatus::Pausing);
    assert_eq!(paused.revision, sandbox.revision + 1);
    assert_eq!(
        paused.lifecycle.paused_notice,
        Some(inactivity_pause_notice(900))
    );
    let events = store.sandbox_events(&sandbox.id, 0, 10).await.unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].seq, 2);
    assert_eq!(
        events[1].kind,
        SandboxEventKind::StatusChanged {
            from: SandboxStatus::Active,
            to: SandboxStatus::Pausing,
        }
    );
    for expected in [first, second] {
        assert_eq!(
            store.get_agent(&expected.id).await.unwrap(),
            Some(expected.clone())
        );
        let document = store
            .agents
            .find_one(doc! { "_id": expected.id.as_str() })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(document.get_i64("sandbox_maintenance_fence").unwrap(), 1);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_rotation_captures_live_identity_and_replays_only_exact_target() {
    let (client, name, store) = database().await;
    let (sandbox, first) = fixture(&store, "sbx_rotation").await;
    let second = ready_agent(&sandbox, "agt_a");
    insert_agent(&store, &second, IDLE_AT).await;
    let mut terminal = ready_agent(&sandbox, "agt_terminal");
    terminal.status = AgentStatus::Stopped;
    insert_agent(&store, &terminal, IDLE_AT).await;
    assert_eq!(
        store
            .schedule_image_rotation(rotation(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::Scheduled
    );
    let stored = store.get_sandbox(&sandbox.id).await.unwrap().unwrap();
    let Some(SandboxLifecycleOperation::ImageRotation { identity, .. }) =
        &stored.lifecycle.operation
    else {
        panic!("rotation operation missing")
    };
    assert_eq!(
        identity
            .agents
            .iter()
            .map(|agent| &agent.agent_id)
            .collect::<Vec<_>>(),
        [&second.id, &first.id]
    );
    assert_eq!(
        store
            .schedule_image_rotation(rotation(&sandbox.id))
            .await
            .unwrap(),
        SandboxMaintenanceOutcome::AlreadyScheduled
    );
    let mut other = rotation(&sandbox.id);
    other.expected_runtime_version = "image-v3".into();
    assert_eq!(
        store.schedule_image_rotation(other).await.unwrap(),
        SandboxMaintenanceOutcome::NotEligible
    );
    assert_eq!(store.get_sandbox(&sandbox.id).await.unwrap(), Some(stored));
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    let terminal_row = store
        .agents
        .find_one(doc! { "_id": terminal.id.as_str() })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        terminal_row.get_i64("sandbox_maintenance_fence").unwrap(),
        1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn maintenance_requests_concurrently_schedule_once() {
    let (client, name, store) = database().await;
    let (sandbox, _) = fixture(&store, "sbx_concurrent").await;
    let store = Arc::new(store);
    let results = futures_util::future::join_all((0..8).map(|_| {
        let store = store.clone();
        let request = pause(&sandbox.id);
        async move { store.schedule_inactivity_pause(request).await.unwrap() }
    }))
    .await;
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == SandboxMaintenanceOutcome::Scheduled)
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|result| **result == SandboxMaintenanceOutcome::AlreadyScheduled)
            .count(),
        7
    );
    assert_eq!(
        store
            .sandbox_events(&sandbox.id, 0, 10)
            .await
            .unwrap()
            .len(),
        1
    );
    client.database(&name).drop().await.unwrap();
}
