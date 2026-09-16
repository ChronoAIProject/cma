//! The admitted original command survives reset; reset never admits a new one.
use super::renewal::lifecycle_tests::lifecycle_fixture::LifecycleFixture;
use super::*;
use layer2_core::bridge::invoke::*;
use layer2_core::sandbox::{agent_io::*, store::*, AgentOperationService};
use mongodb::bson::Document;
use std::time::SystemTime;

#[path = "command_reset_authority_tests.rs"]
mod authority;

async fn reset(
    f: &LifecycleFixture,
    queued: &super::renewal::lifecycle_tests::command_fixture::QueuedChannelFixture,
    original: &AllocationRecord,
) -> AllocationRecord {
    let f = &f.f.original;
    let store = MongoBridgeAllocationStore::open(
        f.bridges.clone(),
        Some(queued.store.clone()),
        &f.database,
    )
    .await
    .unwrap();
    let (admission, authority, management) = proposal(
        &f.svc,
        &f.bridges,
        &original.pin.bridge_id,
        "12345",
        Some(original),
    )
    .await;
    store
        .reserve(
            AllocationReservation::reset(admission, authority, original.clone(), management, now())
                .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn accepted_new_sandbox_command_executes_original_receipt_after_reset() {
    let f = LifecycleFixture::new().await;
    let queued = f
        .queued_channel_command("new-sandbox-original-reset-command")
        .await;
    let original = &f.f.original.record;
    let original_agent =
        f.f.original
            .resources
            .get_agent(&original.placement.target().agent_id)
            .await
            .unwrap()
            .unwrap();
    let next = reset(&f, &queued, original).await;
    let retired =
        f.f.original
            .allocations
            .generation(&original.identity)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(retired.state, AllocationState::Retired);
    let proof = queued
        .channel
        .authorize_execution(&queued.invokes, &queued.command)
        .await
        .unwrap();
    let grant_before = proof.fence().proof().grant().clone();
    assert!(grant_before.channel.as_ref().unwrap().allocation == *original);
    let admission = queued
        .invokes
        .authorize(BridgeInvokeRequest {
            grant_id: grant_before.id.clone(),
            pin: queued.inbox.pin.clone(),
            target: original.placement.target().clone(),
            effect: BridgeInvokeEffect::AdmitMessage {
                inbox_id: queued.inbox.id.clone(),
            },
        })
        .await
        .unwrap();
    assert!(
        ChannelCommandReservation::prepare(admission, queued.inbox.clone(), retired.clone())
            .is_err()
    );
    let basis = grant_before.channel.as_ref().unwrap();
    let mut corrupt = retired.clone();
    corrupt.revision += 1;
    assert!(basis.require_retained_allocation(&corrupt).is_err());
    corrupt = retired.clone();
    corrupt.receipt.as_mut().unwrap().receipt_digest = "f".repeat(64);
    assert!(basis.require_retained_allocation(&corrupt).is_err());
    corrupt = retired.clone();
    corrupt.state = AllocationState::FailedTerminal;
    assert!(basis.require_retained_allocation(&corrupt).is_err());
    let stamp = SystemTime::now();
    let claimed =
        f.f.original
            .resources
            .claim_next_command(CommandClaimRequest {
                agent_id: original_agent.id.clone(),
                holder: "reset-original-command-worker".into(),
                now: layer2_core::rfc3339_at(stamp),
                expires_at: layer2_core::rfc3339_at(stamp + Duration::from_secs(30)),
            })
            .await
            .unwrap()
            .unwrap();
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &claimed)
        .await
        .unwrap();
    AgentOperationService::new(f.f.original.resources.clone(), f.registry.clone())
        .execute_channel_claim(&authority, &claimed)
        .await
        .unwrap();
    let after =
        f.f.original
            .resources
            .get_command(&claimed.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(after.status, AgentResponseStatus::InProgress);
    assert_eq!(after.receipt, queued.command.receipt);
    let after_agent =
        f.f.original
            .resources
            .get_agent(&original_agent.id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(
        after_agent.conversation.conversation_id,
        original_agent.conversation.conversation_id
    );
    assert!(f
        .f
        .original
        .resources
        .get_agent(&next.placement.target().agent_id)
        .await
        .unwrap()
        .is_none());
    assert!(queued.store.grant(&grant_before.id).await.unwrap().unwrap() == grant_before);
    assert!(
        f.f.original
            .allocations
            .current(&next.identity)
            .await
            .unwrap()
            .unwrap()
            == next
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_command_fence_requires_complete_canonical_reset_chain() {
    let f = LifecycleFixture::new().await;
    let queued = f
        .queued_channel_command("reset-chain-original-command")
        .await;
    allow_three_generations(&f).await;
    let second = reset(&f, &queued, &f.f.original.record).await;
    let third = reset(&f, &queued, &second).await;
    let authority = queued
        .channel
        .authorize_execution(&queued.invokes, &queued.command)
        .await
        .unwrap();
    let fence = authority.fence();
    let database = f.f.original.client.database(&f.f.original.database);
    let records = database.collection::<Document>("bridge_allocations");
    let second_row = records
        .find_one(doc! {"_id":&second.id})
        .await
        .unwrap()
        .unwrap();
    let third_row = records
        .find_one(doc! {"_id":&third.id})
        .await
        .unwrap()
        .unwrap();
    for corruption in 0..3 {
        if corruption == 1 {
            records.delete_one(doc! {"_id":&second.id}).await.unwrap();
        } else if corruption == 2 {
            let mut corrupt = third_row.clone();
            let record = corrupt.get_document_mut("record").unwrap();
            record.insert("created_at_ms", third.created_at_ms + 1);
            record.insert("updated_at_ms", third.updated_at_ms + 1);
            records
                .replace_one(doc! {"_id":&third.id}, corrupt)
                .await
                .unwrap();
        }
        let mut session = f.f.original.client.start_session().await.unwrap();
        crate::store::sandbox_mongo::start_transaction(&mut session)
            .await
            .unwrap();
        let result = queued
            .store
            .fence_channel_execution(&fence, &queued.command, &mut session)
            .await;
        assert_eq!(result.is_ok(), corruption == 0);
        session.abort_transaction().await.unwrap();
        if corruption == 1 {
            records.insert_one(second_row.clone()).await.unwrap();
        }
        if corruption == 2 {
            records
                .replace_one(doc! {"_id":&third.id}, third_row.clone())
                .await
                .unwrap();
        }
    }
    assert_eq!(
        f.f.original
            .resources
            .get_command(&queued.command.receipt.envelope.command_id)
            .await
            .unwrap(),
        Some(queued.command)
    );
    f.close().await;
}

async fn allow_three_generations(f: &LifecycleFixture) {
    let f = &f.f.original;
    let bridge = f
        .bridges
        .bridge(&f.record.pin.bridge_id)
        .await
        .unwrap()
        .unwrap();
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                &bridge.id,
                bridge.revision,
                "chain-capacity-disable",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            ),
        )
        .await
        .unwrap();
    let bridge = f.bridges.bridge(&bridge.id).await.unwrap().unwrap();
    let mut config = bridge.current().unwrap().config.clone();
    config.limits.max_partitions = config.limits.max_partitions.max(3);
    config.limits.max_sandboxes = 3;
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                &bridge.id,
                bridge.revision,
                "chain-capacity-update",
                BridgeChange::Update {
                    config: Box::new(config),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            ),
        )
        .await
        .unwrap();
    let bridge = f.bridges.bridge(&bridge.id).await.unwrap().unwrap();
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                &bridge.id,
                bridge.revision,
                "chain-capacity-enable",
                BridgeChange::Enable,
            ),
        )
        .await
        .unwrap();
}
