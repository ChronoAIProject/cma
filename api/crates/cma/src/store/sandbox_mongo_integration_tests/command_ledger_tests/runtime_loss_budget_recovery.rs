use super::fixtures::{ledger, running_with, settled, turn};
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_recovers_saved_policy_and_original_partition_after_disable() {
    for segmented in [false, true] {
        for sharded in [false, true] {
            let (client, name) = database("budget_runtime_saved").await;
            let (store, agent, parent, _) =
                running_with(&client, &name, segmented, sharded, false).await;
            let before = turn(&store, &agent).await;
            store.budget_settings.delete_many(doc! {}).await.unwrap();
            let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
            let request = loss(&parent);
            assert_eq!(
                reopened
                    .recover_runtime_loss(request.clone())
                    .await
                    .unwrap(),
                store::SandboxRuntimeLossOutcome::Failed
            );
            settled(&reopened, &agent, &before).await;
            let after = ledger(&reopened).await;
            assert_eq!(
                reopened.recover_runtime_loss(request).await.unwrap(),
                store::SandboxRuntimeLossOutcome::Superseded
            );
            assert_eq!(ledger(&reopened).await, after);
            client.database(&name).drop().await.unwrap();
        }
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_closes_budget_with_mixed_children_and_concurrent_observers() {
    let (client, name) = database("budget_runtime_mixed").await;
    let (store, agent, parent, _) = running_with(&client, &name, true, true, false).await;
    let sibling = ready_agent(&parent, "agt_unbudgeted_sibling");
    insert_agent(&store, &sibling, NOW).await;
    let before = turn(&store, &agent).await;
    let results =
        futures_util::future::join_all((0..8).map(|_| store.recover_runtime_loss(loss(&parent))))
            .await;
    assert_eq!(
        results
            .iter()
            .filter(|outcome| matches!(outcome, Ok(store::SandboxRuntimeLossOutcome::Failed)))
            .count(),
        1
    );
    assert!(results.iter().all(|outcome| matches!(
        outcome,
        Ok(store::SandboxRuntimeLossOutcome::Failed | store::SandboxRuntimeLossOutcome::Superseded)
    )));
    settled(&store, &agent, &before).await;
    assert_eq!(
        store.get_agent(&sibling.id).await.unwrap().unwrap().status,
        AgentStatus::Stopped
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_preserves_cleanup_hold_until_a_later_terminal_recovery() {
    for segmented in [false, true] {
        let (client, name) = database("budget_runtime_retained_cleanup").await;
        let (store, mut agent, parent, _) =
            running_with(&client, &name, segmented, false, true).await;
        agent.requested_stop_reason = Some(AgentStopReason::Owner);
        store
            .agents
            .update_one(
                doc! {"_id":agent.id.as_str()},
                doc! {"$set": crate::store::agents::agent_replacement_fields(&agent).unwrap()},
            )
            .await
            .unwrap();
        let before = ledger(&store).await;
        let mut request = loss(&parent);
        request.retain_runtime = true;
        let store::SandboxRuntimeLossApplication::Persist(mutation) =
            store::apply_sandbox_runtime_loss(
                &parent,
                &[(agent.clone(), agent.conversation.last_seq)],
                &request,
            )
            .unwrap()
        else {
            panic!("retained cleanup must produce a mutation");
        };
        let change = &mutation.agents[0];
        assert_eq!(
            change.agent.conversation.active_turn,
            agent.conversation.active_turn
        );
        assert!(
            change.agent.conversation.runtime_generation > agent.conversation.runtime_generation
        );
        let mut session = client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        crate::store::budget::settle_runtime_loss(
            &store,
            &agent,
            change,
            &request.observed_at,
            &mut session,
        )
        .await
        .unwrap();
        session.commit_transaction().await.unwrap();
        assert_eq!(ledger(&store).await, before);
        let outcome = store.recover_runtime_loss(request).await;
        if segmented {
            // Segmented projection refuses retirement while a turn is active.
            assert!(
                matches!(outcome, Err(store::SandboxStoreError::Corrupt(_))),
                "{outcome:?}"
            );
            assert_eq!(
                store.get_agent(&agent.id).await.unwrap().as_ref(),
                Some(&agent)
            );
        } else {
            assert_eq!(
                outcome.unwrap(),
                store::SandboxRuntimeLossOutcome::RecoveryScheduled
            );
            let next = store.get_agent(&agent.id).await.unwrap().unwrap();
            assert_eq!(
                next.conversation.active_turn,
                agent.conversation.active_turn
            );
            assert!(next.conversation.runtime_generation > agent.conversation.runtime_generation);
        }
        assert_eq!(ledger(&store).await, before);
        if !segmented {
            let obligation = turn(&store, &agent).await;
            let mut retiring = store.get_sandbox(&parent.id).await.unwrap().unwrap();
            let revision = retiring.revision;
            retiring
                .begin_retirement(layer2_core::now_rfc3339())
                .unwrap();
            // Model the later replacement binding before observing its loss.
            // The retained turn still belongs to the original generation.
            let generation = store
                .get_agent(&agent.id)
                .await
                .unwrap()
                .unwrap()
                .conversation
                .runtime_generation
                .unwrap();
            let mut replacement = *retiring.lifecycle.suspended_runtime.take().unwrap();
            replacement.provision_generation = Some(generation);
            replacement.bound_at = layer2_core::now_rfc3339();
            retiring.runtime = Some(replacement);
            retiring.lifecycle.runtime_generation_high_water = generation;
            store
                .save_sandbox(revision, retiring.clone())
                .await
                .unwrap();
            let request = loss(&retiring);
            assert_eq!(
                store.recover_runtime_loss(request.clone()).await.unwrap(),
                store::SandboxRuntimeLossOutcome::RecoveryScheduled
            );
            settled(&store, &agent, &obligation).await;
            let after = ledger(&store).await;
            store.recover_runtime_loss(request).await.unwrap();
            assert_eq!(ledger(&store).await, after);
        }
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_FAILPOINT_URI pointing at a failpoint-enabled replica set"]
async fn suspended_reopen_closes_saved_turn_in_its_publication_transaction() {
    for segmented in [false, true] {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let admin = Client::with_uri_str(&uri).await.unwrap();
        let name = format!("budget_suspended_{}", uuid::Uuid::new_v4().simple());
        let app = format!("suspended-{name}");
        let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
        options.app_name = Some(app.clone());
        let client = Client::with_options(options).unwrap();
        let (store, agent, mut parent, _) =
            running_with(&client, &name, segmented, true, true).await;
        let before = turn(&store, &agent).await;
        parent.status = SandboxStatus::Failed;
        parent.failure = Some(TypedFailure {
            class: "runtime_lost".into(),
            detail: "retained suspended runtime".into(),
        });
        parent
            .work
            .schedule(SandboxWorkStep::None, layer2_core::now_rfc3339());
        parent.revision += 1;
        store
            .save_sandbox(parent.revision - 1, parent.clone())
            .await
            .unwrap();
        // The retained fixture has a terminal command receipt with an interrupted
        // runtime whose active turn has not yet been settled by reopening.
        let row = store
            .agent_commands
            .find_one(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut command = crate::store::agent_commands::records::read(&row).unwrap();
        command.status = AgentResponseStatus::Failed;
        command.work.lease = None;
        store
            .agent_commands
            .replace_one(
                doc! {"_id":command.receipt.envelope.command_id.as_str()},
                crate::store::agent_commands::records::document(&command).unwrap(),
            )
            .await
            .unwrap();
        let snapshot = store
            .retained_suspended_reopen_snapshot(&parent.id)
            .await
            .unwrap();
        let source = snapshot.suspended_source().unwrap();
        let request = RetainedSuspendedReopenRequest {
            repeat_authorized: false,
            proof: RetainedSuspendedReopenProof {
                binding: source.binding.clone(),
                home_identity: source.home_identity,
            },
            snapshot,
            key_sha256: "a".repeat(64),
            requested_at: layer2_core::now_rfc3339(),
        };
        let before_ledger = ledger(&store).await;
        let before_publication = super::atomicity::publication_rows(&store).await;
        admin
            .database("admin")
            .run_command(doc! {
                "configureFailPoint":"failCommand", "mode":{"times":1_i32},
                "data":{"failCommands":["insert"],"errorCode":2_i32,"appName":app},
            })
            .await
            .unwrap();
        let failed = store.admit_suspended_reopen(request.clone()).await;
        admin
            .database("admin")
            .run_command(doc! {"configureFailPoint":"failCommand","mode":"off"})
            .await
            .unwrap();
        assert!(failed.is_err());
        assert_eq!(ledger(&store).await, before_ledger);
        assert_eq!(
            super::atomicity::publication_rows(&store).await,
            before_publication
        );
        let accepted = store.admit_suspended_reopen(request.clone()).await.unwrap();
        settled(&store, &agent, &before).await;
        let after = ledger(&store).await;
        assert_eq!(
            store.admit_suspended_reopen(request).await.unwrap(),
            accepted
        );
        assert_eq!(ledger(&store).await, after);
        client.database(&name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_settles_converted_legacy_partition_and_preserves_queued_hold() {
    let (client, name) = database("budget_runtime_converted").await;
    let (original, agent, parent, policy) = running_with(&client, &name, false, false, false).await;
    let before = turn(&original, &agent).await;
    assert!(!before.contains_key("period_shard"));
    super::super::super::budget_tests::raise_owner_limit(&original, 2).await;
    let store = (*original).clone().with_platform_counter_shards(true);
    let queued = record(
        store
            .reserve_command(live_request(&agent, &parent, "queued-hold"))
            .await
            .unwrap(),
    );
    let key = format!("command:{}", queued.receipt.envelope.command_id);
    let queued_hold = store
        .model_budget
        .find_one(doc! {"_id":&key})
        .await
        .unwrap()
        .unwrap();
    let queued_record = queued_hold.get_document("record").unwrap();
    let shard = queued_record
        .get("period_shard")
        .and_then(|value| value.as_i64().or_else(|| value.as_i32().map(i64::from)))
        .unwrap();
    assert!(shard > 0);
    store.budget_settings.delete_many(doc! {}).await.unwrap();
    let reopened = MongoSandboxStore::open(&client, &name).await.unwrap();
    reopened.recover_runtime_loss(loss(&parent)).await.unwrap();
    let mut expected = before.clone();
    expected
        .get_document_mut("reservation")
        .unwrap()
        .insert("settled", true);
    assert_eq!(turn(&reopened, &agent).await, expected);
    assert_eq!(
        reopened
            .model_budget
            .find_one(doc! {"_id":key})
            .await
            .unwrap()
            .unwrap(),
        queued_hold
    );
    let charged = owner(&reopened, &agent).await;
    assert_eq!(charged.reserved, policy.turn_limit);
    assert_eq!(charged.spent, policy.turn_limit);
    let period = before.get_i64("period_start").unwrap();
    for (partition, spent, reserved) in [(0, policy.turn_limit, 0), (shard, 0, policy.turn_limit)] {
        let row = reopened
            .model_budget
            .find_one(doc! {"_id":format!("platform-counter:{period}:{partition}")})
            .await
            .unwrap()
            .unwrap();
        let counter = row.get_document("record").unwrap();
        assert_eq!(counter.get_i64("spent").unwrap(), spent as i64);
        assert_eq!(counter.get_i64("reserved").unwrap(), reserved as i64);
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn runtime_loss_without_a_saved_hold_does_not_create_accounting() {
    let (client, name) = database("budget_runtime_no_hold").await;
    let (store, agent, parent, execution, _, _) = composed(&client, &name).await;
    let admitted = record(
        store
            .reserve_command(live_request(&agent, &parent, "unbudgeted"))
            .await
            .unwrap(),
    );
    AgentCommandWorker::new(store.clone(), execution, "recovery-worker".into())
        .unwrap()
        .run_one(&admitted.receipt.envelope.command_id)
        .await
        .unwrap();
    assert!(store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .is_some());
    let before = ledger(&store).await;
    assert!(before.is_empty());
    store.recover_runtime_loss(loss(&parent)).await.unwrap();
    assert!(store
        .get_agent(&agent.id)
        .await
        .unwrap()
        .unwrap()
        .conversation
        .active_turn
        .is_none());
    assert_eq!(ledger(&store).await, before);
    client.database(&name).drop().await.unwrap();
}
