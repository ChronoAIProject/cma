//! Exercise the handoff after the atomic Ready commit, including races and recovery.
use super::*;
use layer2_core::sandbox::store::{AgentWorkerStore, SandboxStore};

mod capacity_tests;

fn at() -> SystemTime {
    SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(
            layer2_core::seconds_from_rfc3339("2026-09-05T00:00:00Z").unwrap(),
        )
}

async fn finish(fixture: &Fixture, context: &AgentContext) {
    fixture
        .worker
        .publish_prepared_connection(&fixture.agent, context, &fixture.attachment, at())
        .await
        .unwrap();
}

fn target(fixture: &Fixture) -> LiveAgentTarget {
    let identity = fixture.attachment.identity();
    LiveAgentTarget {
        agent_id: identity.agent_id,
        conversation_id: identity.conversation_id,
        runtime_generation: identity.runtime_generation,
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_claims_new_authority_and_preserves_queued_first_message() {
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    let mut ready = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let revision = ready.revision;
    ready.conversation.first_message = Some(
        FirstMessageRecord::new(&ready.creation_key, "first prompt", "2026-09-05T00:00:00Z")
            .unwrap(),
    );
    ready.revision += 1;
    fixture
        .store
        .save_agent(revision, ready.clone())
        .await
        .unwrap();
    finish(&fixture, &fixture.context).await;
    let done = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.work.step, AgentWorkStep::DispatchFirstMessage);
    assert_eq!(
        done.conversation.first_message,
        ready.conversation.first_message
    );
    assert!(done.work.lease.is_none());
    assert_eq!(done.work.next_fence, fixture.agent.work.next_fence + 1);
    let attachment = fixture
        .worker
        .dependencies
        .registry
        .attachment(&target(&fixture), done.work.next_fence - 1)
        .unwrap()
        .unwrap();
    assert_eq!(
        attachment.identity().nonce,
        fixture.attachment.identity().nonce
    );
    assert!(fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&target(&fixture)));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_changed_observation_cannot_publish_staged_channel() {
    for case in 0..6 {
        let fixture = Fixture::new().await;
        assert!(fixture.publish().await.unwrap());
        let mut observed = AgentContext {
            agent_resource_preparation: false,
            sandbox: fixture.context.sandbox.clone(),
            workspace: fixture.context.workspace.clone(),
            worktree: fixture.context.worktree.clone(),
            runtime: fixture.context.runtime.clone(),
        };
        match case {
            0 => observed.sandbox.runtime.as_mut().unwrap().boot_id = Some("d".repeat(32)),
            1 => {
                observed
                    .sandbox
                    .lifecycle
                    .home
                    .as_mut()
                    .unwrap()
                    .request
                    .volume_id = "other".into()
            }
            2 => observed.sandbox.credential.delivered = false,
            3 => observed.sandbox.owner = layer2_core::SubjectId::new("other"),
            4 => observed.workspace.revision += 1,
            _ => observed.worktree.revision += 1,
        }
        finish(&fixture, &observed).await;
        // The fixture has no adopted runtime: only a full fallback is allowed
        // to fail/retry here; the staged connection must not become public.
        let current = fixture
            .store
            .get_agent(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            current.work.step,
            AgentWorkStep::PublishConnection,
            "case {case}"
        );
        assert!(current.work.lease.is_none(), "case {case}");
        assert!(!fixture
            .worker
            .dependencies
            .registry
            .published_target_is_attached(&target(&fixture)));
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_does_not_steal_another_workers_claim() {
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    let claimed = fixture
        .store
        .claim_agent_work(
            &fixture.agent.id,
            "other-worker",
            rfc3339_at(at()),
            rfc3339_at(at() + std::time::Duration::from_secs(60)),
        )
        .await
        .unwrap()
        .unwrap();
    finish(&fixture, &fixture.context).await;
    assert_eq!(
        fixture
            .store
            .get_agent(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap(),
        claimed
    );
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&target(&fixture)));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_parent_pause_keeps_full_inspection_pending() {
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    let mut parent = fixture.context.sandbox.clone();
    parent
        .begin_pause("2026-09-05T00:00:00Z".into(), None)
        .unwrap();
    fixture
        .store
        .save_sandbox(fixture.context.sandbox.revision, parent)
        .await
        .unwrap();
    finish(&fixture, &fixture.context).await;
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&target(&fixture)));
    let current = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.work.step, AgentWorkStep::PublishConnection);
    assert!(current.work.lease.is_none());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_missing_local_stream_uses_normal_recovery() {
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    fixture
        .worker
        .dependencies
        .registry
        .rollback(&fixture.attachment);
    finish(&fixture, &fixture.context).await;
    let current = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.work.step, AgentWorkStep::PublishConnection);
    assert!(current.work.lease.is_none());
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&target(&fixture)));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_changed_settings_or_open_receipt_uses_fresh_recovery() {
    for settings in [false, true] {
        let fixture = Fixture::new().await;
        assert!(fixture.publish().await.unwrap());
        let mut agent = fixture
            .store
            .get_agent(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap();
        if settings {
            agent.conversation.settings = Some(agent.settings());
        } else {
            agent.work.receipts.last_mut().unwrap().request_sha256 = "d".repeat(64);
        }
        agent.revision += 1;
        // Inject the race at the storage boundary. Generic save_agent correctly
        // refuses edits to the immutable receipt and owner-controlled settings.
        fixture
            .client
            .database(&fixture.name)
            .collection::<mongodb::bson::Document>("agents")
            .update_one(
                mongodb::bson::doc! {"_id": agent.id.as_str()},
                mongodb::bson::doc! {"$set": {
                    "record": mongodb::bson::serialize_to_bson(&agent).unwrap(),
                    "revision": i64::try_from(agent.revision).unwrap(),
                }},
            )
            .await
            .unwrap();
        finish(&fixture, &fixture.context).await;
        let current = fixture
            .store
            .get_agent(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.work.step, AgentWorkStep::PublishConnection);
        assert!(current.work.lease.is_none());
        assert!(!fixture
            .worker
            .dependencies
            .registry
            .published_target_is_attached(&target(&fixture)));
        fixture.cleanup().await;
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_replaced_channel_nonce_cannot_reuse_original_observation() {
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    fixture
        .worker
        .dependencies
        .registry
        .rollback(&fixture.attachment);
    let (_, receiver) = tokio::sync::mpsc::channel(4);
    let replacement = fixture
        .worker
        .dependencies
        .registry
        .attach(
            fixture.agent.id.clone(),
            fixture.attachment.identity().conversation_id,
            1,
            fixture.attachment.identity().authority_fence,
            fixture.agent.capabilities.clone(),
            ConversationInputSender::spawn(4, |_| async { Ok(()) }),
            receiver,
            fixture.worker.dependencies.updates.clone(),
        )
        .unwrap();
    assert_ne!(
        replacement.identity().nonce,
        fixture.attachment.identity().nonce
    );
    finish(&fixture, &fixture.context).await;
    let current = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.work.step, AgentWorkStep::PublishConnection);
    assert!(current.work.lease.is_none());
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&target(&fixture)));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_connection_executes_concurrent_stop_instead_of_stranding_its_lease() {
    use layer2_core::sandbox::store::{AgentControlStore, AgentControlTarget, AgentStopRequest};
    let fixture = Fixture::new().await;
    assert!(fixture.publish().await.unwrap());
    let ready = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    let now = SystemTime::now();
    fixture
        .store
        .schedule_agent_stop(AgentStopRequest {
            target: AgentControlTarget::from(&ready),
            reason: layer2_core::sandbox::AgentStopReason::Owner,
            requested_at: rfc3339_at(now),
        })
        .await
        .unwrap();
    fixture
        .worker
        .publish_prepared_connection(&fixture.agent, &fixture.context, &fixture.attachment, now)
        .await
        .unwrap();
    let current = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.work.step, AgentWorkStep::CloseProcess);
    assert!(current.work.lease.is_none());
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .target_is_attached(&target(&fixture)));
    fixture.cleanup().await;
}
