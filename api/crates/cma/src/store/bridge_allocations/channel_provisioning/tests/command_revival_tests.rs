use super::renewal::lifecycle_tests::lifecycle_fixture::LifecycleFixture;
use super::*;
use crate::bridge::provisioning::revival::ChannelRevivalAuthority;
use layer2_core::sandbox::{
    agent_io::*,
    store::{AgentLogStore, AgentStore},
    AgentOperationService, Sandbox, SandboxStatus,
};
use mongodb::bson::Document;
use std::time::SystemTime;

async fn permit(f: &LifecycleFixture, sandbox: &Sandbox) -> ChannelRevivalAuthority {
    let original = &f.f.original;
    ChannelRevivalAuthority::fixture(
        original.record.clone(),
        renew_retained(&original.svc, &original.bridges, &original.record)
            .await
            .unwrap(),
        original.lease.clone(),
        sandbox.clone(),
    )
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn exact_channel_head_schedules_one_revival_with_human_audit_and_preserved_command() {
    let f = LifecycleFixture::new().await;
    let paused = f.pause().await;
    let queued = f.queued_channel_command("command-revival-head").await;
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &queued.command)
        .await
        .unwrap();
    let permit = permit(&f, &paused).await;
    let fence = authority.fence();
    let requests = f.f.original.backend.requests().await;
    let logs =
        f.f.original
            .resources
            .agent_log(&queued.command.receipt.envelope.agent_id, 0, 100)
            .await
            .unwrap();
    let (one, two) = tokio::join!(
        queued.store.schedule_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &queued.command
        ),
        queued.store.schedule_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &queued.command
        ),
    );
    assert_ne!(
        one.is_ok(),
        two.is_ok(),
        "only one original Paused transition may commit"
    );
    let revived = f.sandbox().await;
    assert_eq!(revived.status, SandboxStatus::Reviving);
    assert_eq!(revived.id, paused.id);
    assert_eq!(revived.revision, paused.revision + 1);
    assert_eq!(revived.runtime, paused.runtime);
    assert_eq!(
        f.f.original
            .resources
            .get_command(&queued.command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(queued.command.clone())
    );
    assert_eq!(
        f.f.original
            .resources
            .agent_log(&queued.command.receipt.envelope.agent_id, 0, 100)
            .await
            .unwrap(),
        logs
    );
    assert_eq!(f.f.original.backend.requests().await, requests);
    let audits =
        f.f.original
            .client
            .database(&f.f.original.database)
            .collection::<Document>("channel_command_revival_audits");
    assert_eq!(audits.count_documents(doc! {}).await.unwrap(), 1);
    let audit = audits.find_one(doc! {}).await.unwrap().unwrap();
    assert_eq!(
        audit.get_str("command_id").unwrap(),
        queued.command.receipt.envelope.command_id.as_str()
    );
    assert_eq!(
        audit.get("human_actor"),
        Some(
            &crate::store::sandbox_mongo::record_bson(&f.f.original.lease.actor_context().actor)
                .unwrap()
        )
    );
    assert!(!format!("{audit:?}").contains(HUMAN_SECRET));
    assert!(!format!("{audit:?}").contains(RUNTIME_SECRET));
    assert!(queued
        .store
        .schedule_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &queued.command
        )
        .await
        .is_err());
    assert_eq!(audits.count_documents(doc! {}).await.unwrap(), 1);
    for _ in 0..10 {
        f.step().await;
    }
    assert_eq!(f.sandbox().await.status, SandboxStatus::Active);
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
                agent_id: queued.command.receipt.envelope.agent_id.clone(),
                holder: "revived-command-worker".into(),
                now: layer2_core::rfc3339_at(stamp),
                expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(30)),
            })
            .await
            .unwrap()
            .unwrap();
    assert_eq!(claimed.receipt, queued.command.receipt);
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &claimed)
        .await
        .unwrap();
    let operations = AgentOperationService::new(f.f.original.resources.clone(), f.registry.clone());
    operations
        .execute_channel_claim(&authority, &claimed)
        .await
        .unwrap();
    assert!(operations
        .execute_channel_claim(&authority, &claimed)
        .await
        .is_err());
    let after =
        f.f.original
            .resources
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(after.status, AgentResponseStatus::InProgress);
    assert_eq!(after.receipt, queued.command.receipt);
    let active =
        f.f.original
            .resources
            .get_agent(&claimed.receipt.envelope.agent_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        active.conversation.active_turn,
        after.work.dispatch.as_ref().map(|d| d.turn_sequence)
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn held_earlier_channel_message_prevents_later_command_from_reviving_the_parent() {
    let f = LifecycleFixture::new().await;
    let paused = f.pause().await;
    let head = f.queued_channel_command("revival-first").await;
    let later = f.queued_channel_command("revival-later").await;
    let authority = later
        .channel
        .authorize_execution(&later.invokes, &later.command)
        .await
        .unwrap();
    let permit = permit(&f, &paused).await;
    let fence = authority.fence();
    assert!(later
        .store
        .schedule_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &later.command
        )
        .await
        .is_err());
    // Model a retained held head with its exact original receipt and source.
    // The actual signed hold/release transition is covered by admission tests.
    let mut held = head.command.clone();
    held.status = AgentResponseStatus::WaitingAction;
    let mut fields = crate::store::agent_commands::command_document(&held).unwrap();
    fields.remove("_id");
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
        .schedule_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &later.command
        )
        .await
        .is_err());
    assert_eq!(f.sandbox().await, paused);
    let audits =
        f.f.original
            .client
            .database(&f.f.original.database)
            .collection::<Document>("channel_command_revival_audits");
    assert_eq!(audits.count_documents(doc! {}).await.unwrap(), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn changed_or_expired_channel_command_cannot_reuse_a_paused_revival_proof() {
    let f = LifecycleFixture::new().await;
    let paused = f.pause().await;
    let queued = f.queued_channel_command("revival-stale-command").await;
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &queued.command)
        .await
        .unwrap();
    let permit = permit(&f, &paused).await;
    let fence = authority.fence();
    let mut expired = queued.command.clone();
    expired.receipt.admitted_at =
        layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(600));
    expired.queue_deadline = layer2_core::rfc3339_at(SystemTime::now() - Duration::from_secs(1));
    assert!(queued
        .store
        .schedule_channel_command_revival(&f.f.original.allocations, &permit, &fence, &expired)
        .await
        .is_err());
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
        .schedule_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &queued.command
        )
        .await
        .is_err());
    assert_eq!(f.sandbox().await, paused);
    let audits =
        f.f.original
            .client
            .database(&f.f.original.database)
            .collection::<Document>("channel_command_revival_audits");
    assert_eq!(audits.count_documents(doc! {}).await.unwrap(), 0);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn removal_of_original_human_grant_blocks_channel_command_revival_atomically() {
    let f = LifecycleFixture::new().await;
    let paused = f.pause().await;
    let queued = f.queued_channel_command("revival-missing-human").await;
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &queued.command)
        .await
        .unwrap();
    let permit = permit(&f, &paused).await;
    let fence = authority.fence();
    let requests = f.f.original.backend.requests().await;
    let db = f.f.original.client.database(&f.f.original.database);
    let events = db.collection::<Document>("sandbox_events");
    let event_count = events.count_documents(doc! {}).await.unwrap();
    db.run_command(
        doc! {"collMod":"channel_command_revival_audits", "validator":{"forced_failure":true},
        "validationLevel":"strict","validationAction":"error"},
    )
    .await
    .unwrap();
    assert!(queued
        .store
        .schedule_channel_command_revival(
            &f.f.original.allocations,
            &permit,
            &fence,
            &queued.command
        )
        .await
        .is_err());
    assert_eq!(f.sandbox().await, paused);
    assert_eq!(events.count_documents(doc! {}).await.unwrap(), event_count);
    db.run_command(doc! {"collMod":"channel_command_revival_audits","validator":{}})
        .await
        .unwrap();
    f.f.original
        .client
        .database(&f.f.original.database)
        .collection::<Document>("bridge_credentials")
        .update_many(doc! {}, doc! {"$unset":{"control_plane":""}})
        .await
        .unwrap();
    assert!(queued
        .store
        .schedule_channel_command_revival(
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
    assert_eq!(f.f.original.backend.requests().await, requests);
    let audits =
        f.f.original
            .client
            .database(&f.f.original.database)
            .collection::<Document>("channel_command_revival_audits");
    assert_eq!(audits.count_documents(doc! {}).await.unwrap(), 0);
    f.close().await;
}
