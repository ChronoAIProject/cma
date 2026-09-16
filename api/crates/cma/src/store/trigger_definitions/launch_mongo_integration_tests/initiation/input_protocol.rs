use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn common_launch_selector_survives_reopen_and_flag_change_without_relabeling_legacy() {
    let mut f = Fixture::new(false).await;
    let legacy = f.launch("legacy-selection").await;
    f.service = f.service.with_common_first_messages(true);
    let replay = f.launch("legacy-selection").await;
    assert_eq!(replay, legacy);
    assert!(replay.input_protocol.is_none());
    let common = f.launch("common-selection").await;
    assert!(common.input_protocol.is_some());
    let protocols = f
        .seed
        .client
        .database(&f.seed.database)
        .collection::<Document>("trigger_launch_input_protocols");
    assert_eq!(protocols.count_documents(doc! {}).await.unwrap(), 1);
    let reopened = MongoTriggerDefinitionStore::open(
        &f.seed.client,
        &f.seed.database,
        f.seed.profiles.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.launch(&common.id, &common.owner).await.unwrap(),
        Some(common.clone())
    );
    f.service = f.service.with_common_first_messages(false);
    assert_eq!(f.launch("common-selection").await, common);
    let advanced = f.ready_for_agent(common.clone()).await;
    assert_eq!(advanced.input_protocol, common.input_protocol);
    let mut removal = advanced.clone();
    removal.input_protocol = None;
    removal.revision += 1;
    assert!(f
        .seed
        .store
        .commit_launch(LaunchCommit {
            expected_revision: advanced.revision,
            launch: removal,
            event_at: layer2_core::now_rfc3339(),
            events: vec![],
        })
        .await
        .is_err());
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn common_launch_missing_sidecar_or_receipt_blocks_replay_and_worker_claim() {
    for lose_sidecar in [true, false] {
        let mut f = Fixture::new(false).await;
        f.service = f.service.with_common_first_messages(true);
        let launch = f.launch("lost-protocol-proof").await;
        let database = f.seed.client.database(&f.seed.database);
        if lose_sidecar {
            database
                .collection::<Document>("trigger_launches")
                .update_one(
                    doc! {"_id": launch.id.as_str()},
                    doc! {"$unset": {"trigger_input_protocol": ""}},
                )
                .await
                .unwrap();
        } else {
            database
                .collection::<Document>("trigger_launch_input_protocols")
                .delete_one(doc! {"_id": launch.id.as_str()})
                .await
                .unwrap();
        }
        assert!(matches!(
            f.seed.store.launch(&launch.id, &launch.owner).await,
            Err(LaunchStoreError::Corrupt(_))
        ));
        assert!(matches!(
            f.seed
                .store
                .launch_by_command(&launch.owner, &launch.public_id, &launch.command_key_hash,)
                .await,
            Err(LaunchStoreError::Corrupt(_))
        ));
        let now = layer2_core::now_rfc3339();
        assert!(matches!(
            f.seed
                .store
                .claim_launch(
                    &launch.id,
                    "common-worker",
                    now,
                    layer2_core::rfc3339_at(
                        std::time::SystemTime::now() + std::time::Duration::from_secs(60)
                    ),
                )
                .await,
            Err(LaunchStoreError::Corrupt(_))
        ));
        f.service = f.service.with_common_first_messages(false);
        assert!(f
            .service
            .create_or_resume_as(
                &f.identity,
                &f.public.public_id,
                f.request("lost-protocol-proof"),
            )
            .await
            .is_err());
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn common_launch_requires_initiation_and_empty_first_message_creates_no_command() {
    let mut f = Fixture::new(false).await;
    f.service = f.service.with_common_first_messages(true);
    let launch = f
        .ready_for_agent(f.launch("common-unsupported-worker").await)
        .await;
    let request = f.admission(&launch).await;
    assert!(f.resources.admit_agent(request.clone()).await.is_err());
    assert!(f
        .resources
        .admit_agent_reusing_profile(request.clone())
        .await
        .is_err());
    let created = f
        .resources
        .admit_agent_with_trigger_initiator(
            request.clone(),
            launch.initiation.clone().unwrap(),
            false,
        )
        .await
        .unwrap();
    assert!(created
        .creation()
        .agent
        .conversation
        .first_message
        .is_none());
    assert_eq!(
        f.seed
            .client
            .database(&f.seed.database)
            .collection::<Document>("agent_commands")
            .count_documents(doc! {"agent_id":request.agent_id.as_str()})
            .await
            .unwrap(),
        0
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn common_launch_receipt_failure_rolls_back_launch_events_and_trigger_counter() {
    let mut f = Fixture::new(false).await;
    f.service = f.service.with_common_first_messages(true);
    let database = f.seed.client.database(&f.seed.database);
    database
        .run_command(doc! {
            "collMod":"trigger_launch_input_protocols", "validator":{"force_failure":true},
            "validationLevel":"strict", "validationAction":"error",
        })
        .await
        .unwrap();
    assert!(f
        .service
        .create_or_resume_as(
            &f.identity,
            &f.public.public_id,
            f.request("atomic-protocol"),
        )
        .await
        .is_err());
    for collection in [
        "trigger_launches",
        "trigger_launch_events",
        "trigger_launch_input_protocols",
    ] {
        assert_eq!(
            database
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
    }
    let stats = f
        .seed
        .store
        .trigger(&f.public.trigger_id, &f.seed.owner)
        .await
        .unwrap()
        .unwrap()
        .stats;
    assert_eq!(stats.launches_total, 0);
    database
        .run_command(doc! {"collMod":"trigger_launch_input_protocols", "validator":{}})
        .await
        .unwrap();
    let launch = f.launch("atomic-protocol").await;
    assert!(launch.input_protocol.is_some());
    assert_eq!(
        database
            .collection::<Document>("trigger_launch_input_protocols")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    f.close().await;
}
