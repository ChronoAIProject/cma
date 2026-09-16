use super::*;

async fn sibling_channel(c: &ChannelFixture) -> ChannelFixture {
    use crate::auth::persistence::SecretValue;
    use crate::bridge::custody::{
        BridgeCredentialStore, BridgeCustodyMetadata, StoredBridgeCredential,
    };
    let id = BridgeService::creation_id(&c.f.context, "sibling-bridge").unwrap();
    let mut config = c.f.bridge.current().unwrap().config.clone();
    config.connector.custody =
        BridgeSecretRef::new("brsec_00000000000000000000000000000002".into()).unwrap();
    c.f.definitions
        .mutate(
            &c.f.context,
            Some("fixture-credential"),
            BridgeMutation {
                id: id.clone(),
                expected_revision: 0,
                idempotency_key: "sibling-bridge".into(),
                change: BridgeChange::Create {
                    owner: c.f.bridge.owner.clone(),
                    config: Box::new(config.clone()),
                },
            },
        )
        .await
        .unwrap();
    let management =
        c.f.definitions
            .management_lease(&c.f.context, Some("fixture-credential"), &id, 1)
            .await
            .unwrap();
    c.f.bridges
        .credentials
        .admit(
            &management,
            StoredBridgeCredential {
                metadata: BridgeCustodyMetadata {
                    bridge_id: id.clone(),
                    owner: c.f.bridge.owner.clone(),
                    connector: config.connector,
                    revision: 1,
                    key_state_version: 3,
                    scopes: "proxy read write".into(),
                    allowed_service_ids: vec![],
                    allow_all_services: false,
                    allowed_node_ids: vec![],
                    allow_all_nodes: false,
                    created_at: layer2_core::now_rfc3339(),
                    created_by: c.f.context.actor.clone(),
                    revoked_at: None,
                    revoked_by: None,
                    revocation_key_digest: None,
                },
                value: SecretValue::new("fixture-sibling-secret".into()),
                control_plane: None,
            },
        )
        .await
        .unwrap();
    let bridge = c.f.bridges.bridge(&id).await.unwrap().unwrap();
    ChannelFixture::from_fixture(Fixture {
        client: c.f.client.clone(),
        database: c.f.database.clone(),
        bridges: c.f.bridges.clone(),
        resources: c.f.resources.clone(),
        store: c.f.store.clone(),
        definitions: c.f.definitions.clone(),
        service: c.f.service.clone(),
        bridge,
        target: c.f.target.clone(),
        context: c.f.context.clone(),
        authority: c.f.authority.clone(),
    })
    .await
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_usage_management_retire_preserves_another_bridges_active_binding_on_same_agent() {
    let c = ChannelFixture::new().await;
    c.commands
        .reserve_channel_command(c.request().await)
        .await
        .unwrap();
    let sibling = sibling_channel(&c).await;
    sibling
        .commands
        .reserve_channel_command(sibling.request().await)
        .await
        .unwrap();
    let before = history(&c).await;
    assert_eq!(before.0.active_bindings.len(), 2);
    let sibling_binding = before
        .0
        .active_bindings
        .iter()
        .find(|binding| binding.reference.bridge_id == sibling.f.bridge.id.as_str())
        .unwrap()
        .clone();
    let sibling_allocation = allocation_document(&sibling).await;
    mutate(&c, retire(&c, AcceptedWorkPolicy::DrainOriginal))
        .await
        .unwrap();
    let (after, events) = history(&c).await;
    assert_eq!(after.active_bindings, vec![sibling_binding]);
    assert_eq!(after.creation, before.0.creation);
    assert_eq!(events.len(), before.1.len() + 1);
    assert_eq!(allocation_document(&sibling).await, sibling_allocation);
    c.close().await;
}
