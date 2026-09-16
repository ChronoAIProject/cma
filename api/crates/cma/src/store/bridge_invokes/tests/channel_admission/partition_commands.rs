use super::*;
use layer2_core::bridge::{allocation::*, partition_commands::*};

async fn service(
    c: &ChannelFixture,
) -> (
    BridgePartitionCommandService,
    Arc<crate::store::MongoBridgeAllocationStore>,
) {
    let allocations = Arc::new(
        crate::store::MongoBridgeAllocationStore::open(
            c.f.bridges.clone(),
            Some(c.f.store.clone()),
            &c.f.database,
        )
        .await
        .unwrap(),
    );
    let service = BridgePartitionCommandService::new(
        c.f.definitions.clone(),
        Arc::new(
            crate::bridge::partition_commands::ManagedPartitionCommands::new(
                c.f.definitions.clone(),
                allocations.clone(),
                c.f.service.clone(),
                None,
            ),
        ),
    );
    (service, allocations)
}
fn request(
    c: &ChannelFixture,
    allocation: &AllocationRecord,
    action: PartitionCommandAction,
) -> PartitionCommandRequest {
    PartitionCommandRequest {
        allocation_id: allocation.id.clone(),
        action,
        expected_revision: allocation.revision,
        expected_bridge_revision: c.f.bridge.revision,
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_commands_fixed_agent_keep_native_context_and_accepted_execution_across_detach_reset(
) {
    let c = ChannelFixture::new().await;
    let admitted = c
        .commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let before =
        c.f.resources
            .get_agent(&c.f.target.agent_id)
            .await
            .unwrap()
            .unwrap();
    let channel = ChannelAgentCommandService::new(c.commands.clone());
    let (service, allocations) = service(&c).await;
    c.f.client
        .database(&c.f.database)
        .collection::<Document>("bridge_callback_inbox")
        .delete_one(doc! {"_id":&c.inbox.id})
        .await
        .unwrap();
    service
        .command(
            &c.f.context,
            Some("fixture-credential"),
            &c.f.bridge.id,
            "detach-fixed",
            request(&c, &c.allocation, PartitionCommandAction::Detach),
        )
        .await
        .unwrap();
    let detached = allocations
        .current(&c.allocation.identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(detached.state, AllocationState::Retired);
    let proof = channel
        .authorize_execution(&c.f.service, &admitted)
        .await
        .unwrap();
    channel
        .release_execution_hold(&proof, &admitted)
        .await
        .unwrap();
    let queued =
        c.f.resources
            .get_command(&admitted.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
    assert_eq!(queued.status, AgentResponseStatus::Queued);
    let request = request(&c, &detached, PartitionCommandAction::Reset);
    let receipt = service
        .command(
            &c.f.context,
            Some("fixture-credential"),
            &c.f.bridge.id,
            "reset-fixed",
            request.clone(),
        )
        .await
        .unwrap();
    let next = allocations
        .current(&c.allocation.identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next.state, AllocationState::Ready);
    assert_eq!(next.placement.target(), c.allocation.placement.target());
    assert_eq!(
        c.f.resources
            .get_agent(&c.f.target.agent_id)
            .await
            .unwrap()
            .unwrap()
            .conversation,
        before.conversation
    );
    let proof = channel
        .authorize_execution(&c.f.service, &queued)
        .await
        .unwrap();
    let mut session = c.f.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    c.f.store
        .fence_channel_execution(&proof.fence(), &queued, &mut session)
        .await
        .unwrap();
    session.abort_transaction().await.unwrap();
    c.f.service
        .revoke(
            &c.f.context,
            Some("fixture-credential"),
            c.f.revoke(&c.grant, "revoke-fixed-after-reset"),
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .command(
                &c.f.context,
                Some("fixture-credential"),
                &c.f.bridge.id,
                "reset-fixed",
                request
            )
            .await
            .unwrap(),
        receipt
    );
    assert!(channel
        .authorize_execution(&c.f.service, &queued)
        .await
        .is_err());
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn partition_commands_receipt_recovery_requires_current_organization_admin() {
    let c = ChannelFixture::from_fixture(Fixture::organization().await).await;
    let (service, _) = service(&c).await;
    let request = request(&c, &c.allocation, PartitionCommandAction::Detach);
    let receipt = service
        .command(
            &c.f.context,
            Some("fixture-credential"),
            &c.f.bridge.id,
            "org-detach",
            request.clone(),
        )
        .await
        .unwrap();
    *c.f.authority.role.lock().unwrap() = Some(OrganizationRole::Member);
    assert!(service
        .command(
            &c.f.context,
            Some("fixture-credential"),
            &c.f.bridge.id,
            "org-detach",
            request.clone()
        )
        .await
        .is_err());
    *c.f.authority.role.lock().unwrap() = Some(OrganizationRole::Admin);
    assert_eq!(
        service
            .command(
                &c.f.context,
                Some("fixture-credential"),
                &c.f.bridge.id,
                "org-detach",
                request
            )
            .await
            .unwrap(),
        receipt
    );
    c.close().await;
}
