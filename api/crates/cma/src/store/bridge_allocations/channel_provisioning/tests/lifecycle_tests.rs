use super::*;
use layer2_core::bridge::invoke::BridgeInvokeStore;
#[path = "lifecycle_command_fixture.rs"]
pub(crate) mod command_fixture;
#[path = "lifecycle_fixture.rs"]
pub(crate) mod lifecycle_fixture;
use layer2_core::sandbox::store::AgentStore;
use lifecycle_fixture::LifecycleFixture;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_channel_pause_revive_replaces_scoped_key_and_restores_exact_agent_session_then_renews(
) {
    let f = LifecycleFixture::new().await;
    let agent_id = f.f.original.record.placement.target().agent_id.clone();
    let before =
        f.f.original
            .resources
            .get_agent(&agent_id)
            .await
            .unwrap()
            .unwrap();
    let original_allocation = f.f.original.record.clone();
    for _ in 0..3 {
        f.f.step().await.unwrap();
    }
    f.to_delivery().await;
    for _ in 0..7 {
        f.step().await;
    }
    let active = f.sandbox().await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.work.step, SandboxWorkStep::None);
    assert!(active.lifecycle.operation.is_none());
    assert_eq!(active.credential.key_generation, 3);
    assert!(active.credential.revoke_debt.is_empty());
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 2);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 2);
    assert!(f
        .f
        .original
        .custody
        .get(&active.id, 2)
        .await
        .unwrap()
        .is_none());
    assert_ne!(
        active.runtime.as_ref().unwrap().runtime_id,
        f.active.runtime.as_ref().unwrap().runtime_id
    );
    let after =
        f.f.original
            .resources
            .get_agent(&agent_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(
        after.conversation.conversation_id,
        before.conversation.conversation_id
    );
    assert!(after.conversation.runtime_generation > before.conversation.runtime_generation);
    assert!(
        f.f.original
            .allocations
            .channel_allocation(&original_allocation.id)
            .await
            .unwrap()
            .unwrap()
            == original_allocation
    );
    for _ in 0..3 {
        f.f.step().await.unwrap();
    }
    assert_eq!(f.sandbox().await.credential.key_generation, 4);
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 3);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_revive_unknown_mint_is_held_and_does_not_repeat_post() {
    let f = LifecycleFixture::new().await;
    f.to_delivery().await;
    f.f.state.mint_unknown.store(true, Ordering::SeqCst);
    let _ = f.worker.run_one(&f.active.id, SystemTime::now()).await;
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 1);
    f.f.state.mint_unknown.store(false, Ordering::SeqCst);
    f.release_for_retry().await;
    f.step().await;
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.sandbox().await.credential.key_generation, 1);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    let row =
        f.f.original
            .client
            .database(&f.f.original.database)
            .collection::<Document>("channel_mint_orphans")
            .find_one(doc! {"allocation_id":&f.f.original.record.id})
            .await
            .unwrap()
            .unwrap();
    assert!(row
        .get_document("receipt")
        .unwrap()
        .get_bool("revoked")
        .unwrap());
    f.release_for_retry().await;
    f.step().await;
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_revive_lost_delivery_replays_same_runtime_operation_without_new_mint() {
    let f = LifecycleFixture::new().await;
    f.to_delivery().await;
    f.step().await;
    let pending = f.sandbox().await;
    f.f.original
        .backend
        .lose_next_response(WireOperation::Credential);
    f.step().await;
    f.release_for_retry().await;
    f.step().await;
    assert_eq!(f.sandbox().await.runtime, pending.runtime);
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 1);
    let frames =
        f.f.original
            .backend
            .requests()
            .await
            .into_iter()
            .filter(|(_, frame)| frame.operation() == WireOperation::Credential)
            .collect::<Vec<_>>();
    assert_eq!(frames[frames.len() - 1], frames[frames.len() - 2]);
    assert_eq!(f.sandbox().await.credential.key_generation, 2);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_revive_missing_human_grant_before_delivery_preserves_pending_custody() {
    let f = LifecycleFixture::new().await;
    f.to_delivery().await;
    f.step().await;
    let pending = f.sandbox().await;
    let frames = f.f.original.backend.requests().await.len();
    f.f.original
        .client
        .database(&f.f.original.database)
        .collection::<Document>("bridge_credentials")
        .update_many(doc! {}, doc! {"$unset":{"control_plane":""}})
        .await
        .unwrap();
    f.step().await;
    assert_eq!(f.sandbox().await.credential, pending.credential);
    assert_eq!(f.f.original.backend.requests().await.len(), frames);
    assert!(f
        .f
        .original
        .custody
        .get(&pending.id, 2)
        .await
        .unwrap()
        .is_some());
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_revive_expired_paused_key_is_replaced_before_runtime_delivery() {
    let f = LifecycleFixture::new().await;
    f.pause().await;
    f.expire_paused_key().await;
    let expired = f.sandbox().await.credential.current.unwrap();
    f.revive().await;
    for _ in 0..10 {
        f.step().await;
    }
    let active = f.sandbox().await;
    assert_eq!(active.status, SandboxStatus::Active);
    assert_eq!(active.credential.key_generation, 2);
    assert_ne!(
        active.credential.current.as_ref().unwrap().provider_key_id,
        expired.provider_key_id
    );
    assert!(
        layer2_core::seconds_from_rfc3339(&active.credential.current.unwrap().expires_at).unwrap()
            > SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_secs()
                + 300
    );
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.f.state.revoke_count.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_revival_port_does_not_authorize_image_rotation() {
    let f = LifecycleFixture::new().await;
    let mut sandbox = f.sandbox().await;
    let before = sandbox.clone();
    let identity = layer2_core::sandbox::SandboxRotationIdentity::capture(&sandbox, &[]).unwrap();
    sandbox
        .begin_image_rotation("runtime-v3".into(), identity, layer2_core::now_rfc3339())
        .unwrap();
    let fields = crate::store::sandboxes::sandbox_replacement_fields(&sandbox).unwrap();
    f.f.original
        .client
        .database(&f.f.original.database)
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":sandbox.id.as_str(),"revision":before.revision as i64},
            doc! {"$set":fields},
        )
        .await
        .unwrap();
    let requests = f.f.original.backend.requests().await.len();
    f.step().await;
    let held = f.sandbox().await;
    assert_eq!(held.lifecycle.operation, sandbox.lifecycle.operation);
    assert_eq!(held.runtime, before.runtime);
    assert_eq!(f.f.original.backend.requests().await.len(), requests);
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_signed_channel_message_on_paused_sandbox_is_queued_without_runtime_effects() {
    let f = LifecycleFixture::new().await;
    let paused = f.pause().await;
    let requests = f.f.original.backend.requests().await.len();
    let queued = f.queued_channel_command("paused-channel-input").await;
    assert_eq!(
        queued.command.receipt.envelope.source.reference,
        queued.inbox.id
    );
    assert_eq!(
        queued.command.receipt.envelope.agent_id,
        f.f.original.record.placement.target().agent_id
    );
    queued
        .channel
        .authorize_execution(&queued.invokes, &queued.command)
        .await
        .unwrap();
    assert!(
        queued
            .store
            .channel_target(&f.f.original.record.id)
            .await
            .unwrap()
            .basis
            .allocation
            == f.f.original.record
    );
    assert_eq!(f.sandbox().await.runtime, paused.runtime);
    assert_eq!(f.f.original.backend.requests().await.len(), requests);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_revive_missing_human_grant_holds_before_runtime_creation() {
    let f = LifecycleFixture::new().await;
    f.pause().await;
    f.revive().await;
    let before = f.sandbox().await;
    f.f.original
        .client
        .database(&f.f.original.database)
        .collection::<Document>("bridge_credentials")
        .update_many(doc! {}, doc! {"$unset":{"control_plane":""}})
        .await
        .unwrap();
    f.step().await;
    let held = f.sandbox().await;
    assert_eq!(held.runtime, before.runtime);
    assert_eq!(held.lifecycle.operation, before.lifecycle.operation);
    assert_eq!(held.credential, before.credential);
    assert_eq!(f.f.state.mint_count.load(Ordering::SeqCst), 0);
    assert!(f
        .f
        .original
        .adapter
        .runtime_inventory()
        .await
        .unwrap()
        .is_empty());
    f.close().await;
}
