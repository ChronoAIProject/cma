use super::*;

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_binding_advances_launch_revision_so_stale_commits_conflict() {
    let (f, launch, request) = pending().await;
    let accepted = f
        .resources
        .admit_agent_with_trigger_initiator(request, launch.initiation.clone().unwrap(), false)
        .await
        .unwrap();
    let durable = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(durable.revision, launch.revision + 1);
    let mut stale = launch.clone();
    let fence = stale.work.lease.as_ref().unwrap().fence;
    let at = layer2_core::now_rfc3339();
    stale
        .mark_worker_effect_applied(
            "initiator-worker",
            fence,
            accepted.creation().agent.id.to_string(),
            at.clone(),
        )
        .unwrap();
    assert!(matches!(
        f.seed
            .store
            .commit_launch(LaunchCommit {
                expected_revision: launch.revision,
                launch: stale,
                event_at: at,
                events: vec![]
            })
            .await,
        Err(LaunchStoreError::Conflict { .. })
    ));
    assert_eq!(
        f.seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap(),
        durable
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_admission_rolls_back_command_binding_and_launch_pointer_together() {
    for collection in [
        "agent_commands",
        "trigger_first_message_bindings",
        "trigger_launches",
    ] {
        let (f, launch, request) = pending().await;
        let database = f.seed.client.database(&f.seed.database);
        let validator = if collection == "trigger_launches" {
            doc! {"trigger_input_binding_sha256":{"$exists":false}}
        } else {
            doc! {"forced_failure":true}
        };
        database
            .run_command(doc! {"collMod":collection,"validator":validator,
            "validationLevel":"strict","validationAction":"error"})
            .await
            .unwrap();
        assert!(f
            .resources
            .admit_agent_with_trigger_initiator(
                request.clone(),
                launch.initiation.clone().unwrap(),
                false
            )
            .await
            .is_err());
        assert!(f
            .resources
            .get_agent(&request.agent_id)
            .await
            .unwrap()
            .is_none());
        for name in [
            "agent_commands",
            "trigger_first_message_bindings",
            "agent_command_orders",
        ] {
            assert_eq!(
                database
                    .collection::<Document>(name)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                0,
                "{collection}: {name}"
            );
        }
        assert_eq!(
            f.seed
                .store
                .launch(&launch.id, &launch.owner)
                .await
                .unwrap()
                .unwrap(),
            launch
        );
        database
            .run_command(doc! {"collMod":collection,"validator":{}})
            .await
            .unwrap();
        let accepted = f
            .resources
            .admit_agent_with_trigger_initiator(request, launch.initiation.clone().unwrap(), false)
            .await
            .unwrap();
        let agent = &accepted.creation().agent;
        assert!(agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .common_binding
            .is_some());
        assert_eq!(
            database
                .collection::<Document>("agent_commands")
                .count_documents(doc! {})
                .await
                .unwrap(),
            1
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_replica_race_replays_one_receipt_after_effect_and_cleanup_closure() {
    let (f, launch, request) = pending().await;
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..4 {
        let store = f.resources.clone();
        let request = request.clone();
        let initiation = launch.initiation.clone().unwrap();
        tasks.spawn(async move {
            store
                .admit_agent_with_trigger_initiator(request, initiation, false)
                .await
        });
    }
    let mut receipts = Vec::new();
    while let Some(result) = tasks.join_next().await {
        receipts.push(result.unwrap().unwrap().creation().agent.clone());
    }
    assert!(receipts.iter().all(|agent| agent == &receipts[0]));
    let agent = &receipts[0];
    let binding = agent
        .conversation
        .first_message
        .as_ref()
        .unwrap()
        .common_binding
        .as_ref()
        .unwrap();
    assert_eq!(
        f.resources
            .agent_commands
            .count_documents(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap(),
        1
    );
    let mut changed = request.clone();
    changed.first_message = Some(
        FirstMessageRecord::new(
            changed.creation_key.clone(),
            "Changed accepted input",
            changed.event_at.clone(),
        )
        .unwrap(),
    );
    assert!(f
        .resources
        .admit_agent_with_trigger_initiator(changed, launch.initiation.clone().unwrap(), false)
        .await
        .is_err());
    let mut durable = f
        .seed
        .store
        .launch(&launch.id, &launch.owner)
        .await
        .unwrap()
        .unwrap();
    let revision = durable.revision;
    let fence = durable.work.lease.as_ref().unwrap().fence;
    let at = layer2_core::now_rfc3339();
    durable
        .mark_worker_effect_applied("initiator-worker", fence, agent.id.to_string(), at.clone())
        .unwrap();
    f.seed
        .store
        .commit_launch(LaunchCommit {
            expected_revision: revision,
            launch: durable,
            event_at: at,
            events: vec![],
        })
        .await
        .unwrap();
    f.resources
        .agents
        .update_one(
            doc! {"_id":agent.id.as_str()},
            doc! {"$set":{"channel_reuse_cleanup_admission":{"closed":true}}},
        )
        .await
        .unwrap();
    let replay = f
        .resources
        .admit_agent_with_trigger_initiator(request, launch.initiation.clone().unwrap(), false)
        .await
        .unwrap();
    assert_eq!(replay.creation().agent.id, agent.id);
    assert_eq!(
        replay
            .creation()
            .agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .common_binding
            .as_ref()
            .unwrap()
            .receipt(),
        binding.receipt()
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires an owned disposable Mongo replica set"]
async fn trigger_common_missing_command_receipt_refuses_replay_without_new_selection() {
    let accepted = Accepted::new().await;
    accepted
        .f
        .resources
        .agent_commands
        .delete_one(doc! {"_id":accepted.command.receipt.envelope.command_id.as_str()})
        .await
        .unwrap();
    assert!(accepted
        .f
        .resources
        .admit_agent_with_trigger_initiator(
            accepted.request.clone(),
            accepted.launch.initiation.clone().unwrap(),
            false
        )
        .await
        .is_err());
    assert_eq!(
        accepted
            .f
            .resources
            .agents
            .count_documents(doc! {"creation_key":&accepted.request.creation_key})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        accepted
            .f
            .resources
            .agent_commands
            .count_documents(doc! {"agent_id":accepted.agent.id.as_str()})
            .await
            .unwrap(),
        0
    );
    accepted.f.close().await;
}
