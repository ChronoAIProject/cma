use super::{reuse_command_fixture::ReuseCommandFixture, *};
use layer2_core::sandbox::{agent_io::*, store::*, *};
use mongodb::bson::Document;
use std::time::SystemTime;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reused_channel_child_revives_manual_and_trigger_parents_without_replacing_conversations() {
    for shared_workspace in [false, true] {
        for trigger_parent in [false, true] {
            let f = ReuseCommandFixture::new(shared_workspace, trigger_parent).await;
            let target = f.record.placement.target().clone();
            let child =
                f.f.original
                    .resources
                    .get_agent(&target.agent_id)
                    .await
                    .unwrap()
                    .unwrap();
            let sibling =
                f.f.original
                    .resources
                    .get_agent(&f.f.sibling.id)
                    .await
                    .unwrap()
                    .unwrap();
            let child_origin = f.provenance(&child.id).await;
            let sibling_origin = f.provenance(&sibling.id).await;
            assert!(matches!(
                child_origin.creation_origin,
                AgentCreationOrigin::ChannelBridge { .. }
            ));
            assert_eq!(sibling_origin.creation_origin, AgentCreationOrigin::Manual);
            let paused = f.pause().await;
            let queued = f.queued("reuse-revival-current-head").await;
            let permit = f.permit().await;
            let execution = queued
                .channel
                .authorize_execution(&queued.invokes, &queued.command)
                .await
                .unwrap();
            let fence = execution.fence();
            let requests = f.f.original.backend.requests().await;
            let logs =
                f.f.original
                    .resources
                    .agent_log(&child.id, 0, 100)
                    .await
                    .unwrap();
            let (one, two) = tokio::join!(
                queued.store.schedule_reused_channel_command_revival(
                    &f.f.original.allocations,
                    &permit,
                    &fence,
                    &queued.command
                ),
                queued.store.schedule_reused_channel_command_revival(
                    &f.f.original.allocations,
                    &permit,
                    &fence,
                    &queued.command
                ),
            );
            assert_ne!(
                one.is_ok(),
                two.is_ok(),
                "exactly one paused transition commits: {one:?} {two:?}"
            );
            let reviving = f.sandbox().await;
            assert_eq!(reviving.status, SandboxStatus::Reviving);
            assert_eq!(reviving.revision, paused.revision + 1);
            assert_eq!(reviving.runtime, paused.runtime);
            assert_eq!(f.f.original.backend.requests().await, requests);
            assert_eq!(
                f.f.original
                    .resources
                    .agent_log(&child.id, 0, 100)
                    .await
                    .unwrap(),
                logs
            );
            assert_eq!(
                f.f.original
                    .resources
                    .get_command(&queued.command.receipt.envelope.command_id)
                    .await
                    .unwrap(),
                Some(queued.command.clone())
            );
            let db = f.f.original.client.database(&f.f.original.database);
            let audits = db.collection::<Document>("channel_command_revival_audits");
            assert_eq!(audits.count_documents(doc! {}).await.unwrap(), 1);
            let audit = audits.find_one(doc! {}).await.unwrap().unwrap();
            assert_eq!(audit.get_str("parent_kind").unwrap(), "ordinary_reused");
            assert_eq!(audit.get_str("allocation_id").unwrap(), f.record.id);
            assert_eq!(
                audit.get_str("command_id").unwrap(),
                queued.command.receipt.envelope.command_id.as_str()
            );
            assert!(!format!("{audit:?}").contains(HUMAN_SECRET));
            assert!(!format!("{audit:?}").contains(RUNTIME_SECRET));
            for _ in 0..12 {
                if f.sandbox().await.status == SandboxStatus::Active {
                    break;
                }
                f.step().await;
            }
            let active = f.sandbox().await;
            assert_eq!(active.status, SandboxStatus::Active);
            assert_eq!(active.id, paused.id);
            assert_eq!(active.created_at, paused.created_at);
            assert_eq!(active.origin, paused.origin);
            assert_eq!(active.profile, paused.profile);
            assert_eq!(active.owner, paused.owner);
            assert_eq!(active.credential.current, paused.credential.current);
            assert_eq!(f.provenance(&child.id).await, child_origin);
            assert_eq!(f.provenance(&sibling.id).await, sibling_origin);
            for original in [&child, &sibling] {
                let restored =
                    f.f.original
                        .resources
                        .get_agent(&original.id)
                        .await
                        .unwrap()
                        .unwrap();
                assert_eq!(restored.id, original.id);
                assert_eq!(restored.created_at, original.created_at);
                assert_eq!(restored.workspace_id, original.workspace_id);
                assert_eq!(restored.worktree_id, original.worktree_id);
                assert_eq!(restored.profile, original.profile);
                assert_eq!(
                    restored.conversation.conversation_id,
                    original.conversation.conversation_id
                );
                assert_eq!(restored.conversation.active_turn, None);
                assert_eq!(restored.status, AgentStatus::Ready);
            }
            assert_eq!(
                f.f.original
                    .resources
                    .get_command(&queued.command.receipt.envelope.command_id)
                    .await
                    .unwrap(),
                Some(queued.command.clone())
            );
            let stamp = SystemTime::now();
            let claimed =
                f.f.original
                    .resources
                    .claim_next_command(CommandClaimRequest {
                        agent_id: child.id.clone(),
                        holder: "reused-revived-command-worker".into(),
                        now: layer2_core::rfc3339_at(stamp),
                        expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(30)),
                    })
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(claimed.receipt, queued.command.receipt);
            let execution = queued
                .channel
                .authorize_execution(&queued.invokes, &claimed)
                .await
                .unwrap();
            let operations =
                AgentOperationService::new(f.f.original.resources.clone(), f.registry.clone());
            operations
                .execute_channel_claim(&execution, &claimed)
                .await
                .unwrap();
            assert!(operations
                .execute_channel_claim(&execution, &claimed)
                .await
                .is_err());
            let dispatched =
                f.f.original
                    .resources
                    .get_command(&claimed.receipt.envelope.command_id)
                    .await
                    .unwrap()
                    .unwrap();
            assert_eq!(dispatched.status, AgentResponseStatus::InProgress);
            assert_eq!(dispatched.receipt, queued.command.receipt);
            f.reply_mid_turn(&queued).await;
            assert_eq!(
                f.f.original.server.state.mint_count.load(Ordering::SeqCst),
                1
            );
            assert_eq!(
                db.collection::<Document>("bridge_allocations")
                    .count_documents(doc! {"sandbox_admission":{"$exists":true}})
                    .await
                    .unwrap(),
                0
            );
            f.close().await;
        }
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reused_revival_cannot_overtake_queued_or_held_channel_heads() {
    for shared_workspace in [false, true] {
        let f = ReuseCommandFixture::new(shared_workspace, false).await;
        let paused = f.pause().await;
        let head = f.queued("reuse-revival-first").await;
        let later = f.queued("reuse-revival-later").await;
        let permit = f.permit().await;
        let execution = later
            .channel
            .authorize_execution(&later.invokes, &later.command)
            .await
            .unwrap();
        let fence = execution.fence();
        assert!(later
            .store
            .schedule_reused_channel_command_revival(
                &f.f.original.allocations,
                &permit,
                &fence,
                &later.command
            )
            .await
            .is_err());
        let mut held = head.command.clone();
        held.status = AgentResponseStatus::WaitingAction;
        let mut fields = crate::store::agent_commands::command_document(&held).unwrap();
        fields.remove("_id");
        // Fault fixture preserves the already admitted signed receipt/source.
        f.f.original
            .resources
            .agent_commands
            .update_one(
                doc! {"_id":held.receipt.envelope.command_id.as_str()},
                doc! {"$set":fields},
            )
            .await
            .unwrap();
        assert!(later
            .store
            .schedule_reused_channel_command_revival(
                &f.f.original.allocations,
                &permit,
                &fence,
                &later.command
            )
            .await
            .is_err());
        assert_eq!(f.sandbox().await, paused);
        assert_eq!(
            f.f.original
                .client
                .database(&f.f.original.database)
                .collection::<Document>("channel_command_revival_audits")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reused_revival_rechecks_human_custody_and_rolls_back_failed_audit() {
    let f = ReuseCommandFixture::new(true, true).await;
    let paused = f.pause().await;
    let queued = f.queued("reuse-revival-authority").await;
    let permit = f.permit().await;
    let execution = queued
        .channel
        .authorize_execution(&queued.invokes, &queued.command)
        .await
        .unwrap();
    let fence = execution.fence();
    let db = f.f.original.client.database(&f.f.original.database);
    let events = db.collection::<Document>("sandbox_events");
    let count = events.count_documents(doc! {}).await.unwrap();
    db.run_command(doc! {"collMod":"channel_command_revival_audits","validator":{"forced_failure":true},"validationLevel":"strict","validationAction":"error"}).await.unwrap();
    assert!(queued
        .store
        .schedule_reused_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &queued.command
        )
        .await
        .is_err());
    assert_eq!(events.count_documents(doc! {}).await.unwrap(), count);
    assert_eq!(f.sandbox().await, paused);
    db.run_command(doc! {"collMod":"channel_command_revival_audits","validator":{}})
        .await
        .unwrap();
    db.collection::<Document>("bridge_credentials")
        .update_many(doc! {}, doc! {"$unset":{"control_plane":""}})
        .await
        .unwrap();
    assert!(queued
        .store
        .schedule_reused_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &queued.command
        )
        .await
        .is_err());
    assert_eq!(f.sandbox().await, paused);
    assert_eq!(
        f.f.original
            .resources
            .get_command(&queued.command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(queued.command)
    );
    assert_eq!(
        db.collection::<Document>("channel_command_revival_audits")
            .count_documents(doc! {})
            .await
            .unwrap(),
        0
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reused_revival_rejects_stale_commands_and_new_channel_root_evidence() {
    for reverse_evidence in [false, true] {
        let f = ReuseCommandFixture::new(false, false).await;
        let paused = f.pause().await;
        let queued = f.queued("reuse-revival-origin-fence").await;
        let permit = f.permit().await;
        let execution = queued
            .channel
            .authorize_execution(&queued.invokes, &queued.command)
            .await
            .unwrap();
        let fence = execution.fence();
        let db = f.f.original.client.database(&f.f.original.database);
        if reverse_evidence {
            db.collection::<Document>("bridge_allocations").insert_one(doc! {
                "_id":"fixture-channel-root-evidence", "sandbox_admission":{"sandbox_id":paused.id.as_str()},
            }).await.unwrap();
        } else {
            db.collection::<Document>("sandboxes")
                .update_one(
                    doc! {"_id":paused.id.as_str()},
                    doc! {"$set":{"channel_creation":{"fixture":true}}},
                )
                .await
                .unwrap();
        }
        assert!(queued
            .store
            .schedule_reused_channel_command_revival(
                &f.f.original.allocations,
                &permit,
                &fence,
                &queued.command
            )
            .await
            .is_err());
        assert_eq!(f.sandbox().await, paused);
        if reverse_evidence {
            db.collection::<Document>("bridge_allocations")
                .delete_one(doc! {"_id":"fixture-channel-root-evidence"})
                .await
                .unwrap();
        } else {
            db.collection::<Document>("sandboxes")
                .update_one(
                    doc! {"_id":paused.id.as_str()},
                    doc! {"$unset":{"channel_creation":""}},
                )
                .await
                .unwrap();
        }
        f.f.original
            .resources
            .reject_unclaimed_command(
                &queued.command.receipt.envelope.command_id,
                queued.command.revision,
                CommandFailureCode::AuthorityRevoked,
                layer2_core::now_rfc3339(),
            )
            .await
            .unwrap();
        assert!(queued
            .store
            .schedule_reused_channel_command_revival(
                &f.f.original.allocations,
                &permit,
                &fence,
                &queued.command
            )
            .await
            .is_err());
        assert_eq!(f.sandbox().await, paused);
        assert_eq!(
            db.collection::<Document>("channel_command_revival_audits")
                .count_documents(doc! {})
                .await
                .unwrap(),
            0
        );
        f.close().await;
    }
}
