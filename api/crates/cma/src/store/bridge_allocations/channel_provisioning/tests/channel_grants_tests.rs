//! Every positive issuance starts with the real runtime/child/startup fixture.
//! No Ready aggregate or Channel provenance is inserted directly by these tests.
use super::*;
use crate::bridge::custody::BridgeCredentialStore;
use crate::store::MongoBridgeInvokeStore;
use layer2_core::bridge::invoke::*;
use layer2_core::resource_access::ResourceAccessService;
use mongodb::bson::Document;

async fn invokes(f: &Fixture) -> (Arc<MongoBridgeInvokeStore>, BridgeInvokeService) {
    let store = Arc::new(
        MongoBridgeInvokeStore::open(f.bridges.clone(), f.resources.clone(), &f.database)
            .await
            .unwrap(),
    );
    let service = BridgeInvokeService::new(
        f.svc.clone(),
        Arc::new(ResourceAccessService::new(
            f.resources.clone(),
            Arc::new(Authority),
        )),
        f.resources.clone(),
        store.clone(),
    );
    (store, service)
}

async fn disable(f: &mut Fixture, key: &str) {
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
                key,
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            ),
        )
        .await
        .unwrap();
    f.authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_ready_channel_issues_one_exact_grant_and_replays_after_reopening_store() {
    let mut f = Fixture::new().await;
    f.record = f.ready().await;
    f.authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
    let (store, service) = invokes(&f).await;
    let (_, reopened) = invokes(&f).await;
    let snapshot = store.channel_target(&f.record.id).await.unwrap();
    assert!(snapshot.basis.allocation == f.record);
    assert_eq!(snapshot.basis.allocation.state, AllocationState::Ready);
    let control_plane = Arc::new(f.lease);
    let grant = service
        .issue_channel(
            control_plane.clone(),
            &f.record.id,
            &f.record.pin,
            f.authority.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        grant.consent,
        BridgeSharedContextConsent::UseChannelCreatedAgent
    );
    assert_eq!(grant.target.ids, *f.record.placement.target());
    assert_eq!(grant.issued_by, control_plane.actor_context().actor);
    assert_eq!(
        reopened
            .issue_channel(control_plane, &f.record.id, &f.record.pin, f.authority)
            .await
            .unwrap(),
        grant
    );
    let stored = store.grant(&grant.id).await.unwrap().unwrap();
    assert_eq!(stored.channel.as_deref(), Some(&snapshot.basis));
    let serialized = serde_json::to_string(&stored).unwrap();
    assert!(!serialized.contains(HUMAN_SECRET));
    assert!(!serialized.contains(RUNTIME_SECRET));
    for collection in [
        "bridge_invoke_grants",
        "bridge_invoke_receipts",
        "bridge_invoke_audits",
    ] {
        assert_eq!(
            f.client
                .database(&f.database)
                .collection::<Document>(collection)
                .count_documents(doc! {})
                .await
                .unwrap(),
            1,
            "{collection}"
        );
    }
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_channel_new_inbound_profiles_drain_original_tree_and_suspension_refuses_replay() {
    let mut f = Fixture::new().await;
    f.record = f.ready().await;
    let original = f.record.clone();
    disable(&mut f, "profiles-disable").await;
    let bridge = f.authority.bridge();
    let mut updated = original.config.clone();
    updated.display_name = "Updated inbound profile selection".into();
    let BridgePlacementPolicy::NewSandbox { agent_profile, .. } = &mut updated.placement else {
        unreachable!()
    };
    agent_profile.revision += 1;
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                &bridge.id,
                bridge.revision,
                "profiles-update",
                BridgeChange::Update {
                    config: Box::new(updated.clone()),
                    previous_work: Some(AcceptedWorkPolicy::DrainOriginal),
                },
            ),
        )
        .await
        .unwrap();
    let bridge = f
        .bridges
        .bridge(&original.pin.bridge_id)
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
                "profiles-enable",
                BridgeChange::Enable,
            ),
        )
        .await
        .unwrap();
    f.authority = renew_retained(&f.svc, &f.bridges, &original).await.unwrap();
    let inbound = f.authority.bridge().admission_pin().unwrap();
    assert_ne!(inbound, original.pin);
    let (store, service) = invokes(&f).await;
    let control_plane = Arc::new(f.lease);
    let grant = service
        .issue_channel(
            control_plane.clone(),
            &original.id,
            &inbound,
            f.authority.clone(),
        )
        .await
        .unwrap();
    let stored = store.grant(&grant.id).await.unwrap().unwrap();
    assert_eq!(stored.pin, inbound);
    assert_eq!(stored.config, updated);
    assert!(stored.channel.as_ref().unwrap().allocation == original);
    assert!(
        f.allocations
            .generation(&original.identity)
            .await
            .unwrap()
            .unwrap()
            == original
    );
    let bridge = f.authority.bridge();
    f.svc
        .mutate(
            &caller(),
            None,
            change(
                &bridge.id,
                bridge.revision,
                "profiles-suspend",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::SuspendOriginal,
                },
            ),
        )
        .await
        .unwrap();
    assert!(service
        .issue_channel(control_plane, &original.id, &inbound, f.authority)
        .await
        .is_err());
    assert_eq!(store.grant(&grant.id).await.unwrap().unwrap(), stored);
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_channel_revoked_grant_replay_keeps_terminal_revocation_and_original_identity() {
    let mut f = Fixture::new().await;
    f.record = f.ready().await;
    f.authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
    let (store, service) = invokes(&f).await;
    let control_plane = Arc::new(f.lease);
    let grant = service
        .issue_channel(
            control_plane.clone(),
            &f.record.id,
            &f.record.pin,
            f.authority.clone(),
        )
        .await
        .unwrap();
    let revoked = service
        .revoke(
            control_plane.actor_context(),
            Some(control_plane.credential()),
            RevokeBridgeInvokeGrant {
                grant_id: grant.id.clone(),
                expected_bridge_revision: f.authority.bridge().revision,
                expected_revision: grant.revision,
                idempotency_key: "channel-revoke".into(),
            },
        )
        .await
        .unwrap()
        .result;
    assert_eq!(revoked.id, grant.id);
    assert!(matches!(
        revoked.state,
        BridgeInvokeGrantState::Revoked { .. }
    ));
    assert_eq!(
        service
            .issue_channel(control_plane, &f.record.id, &f.record.pin, f.authority)
            .await
            .unwrap(),
        revoked
    );
    assert_eq!(
        store
            .grant(&grant.id)
            .await
            .unwrap()
            .unwrap()
            .require_active(),
        Err(BridgeInvokeError::Revoked)
    );
    assert_eq!(
        f.client
            .database(&f.database)
            .collection::<Document>("bridge_invoke_grants")
            .count_documents(doc! {})
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.client
            .database(&f.database)
            .collection::<Document>("bridge_invoke_audits")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    f.client.database(&f.database).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn real_channel_exact_human_grant_replacement_refuses_new_issuance_and_replay_at_same_key_revision(
) {
    for issue_first in [false, true] {
        let mut f = Fixture::new().await;
        f.record = f.ready().await;
        disable(&mut f, "replace-grant-disable").await;
        let bridge = f.authority.bridge();
        let management = f
            .svc
            .management_lease(&caller(), None, &bridge.id, bridge.revision)
            .await
            .unwrap();
        let mut material = f
            .bridges
            .credentials
            .get(&f.record.config.connector.custody)
            .await
            .unwrap()
            .unwrap();
        let metadata = material.metadata.clone();
        material
            .control_plane
            .as_mut()
            .unwrap()
            .incarnation
            .push_str("-replacement");
        let (store, service) = invokes(&f).await;
        let control_plane = Arc::new(f.lease);
        let issued = if issue_first {
            Some(
                service
                    .issue_channel(
                        control_plane.clone(),
                        &f.record.id,
                        &f.record.pin,
                        f.authority.clone(),
                    )
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        assert_eq!(
            f.bridges
                .credentials
                .admit(&management, material)
                .await
                .unwrap(),
            metadata
        );
        assert_eq!(
            service
                .issue_channel(control_plane, &f.record.id, &f.record.pin, f.authority)
                .await,
            Err(BridgeInvokeError::Unavailable)
        );
        for collection in [
            "bridge_invoke_grants",
            "bridge_invoke_receipts",
            "bridge_invoke_audits",
        ] {
            assert_eq!(
                f.client
                    .database(&f.database)
                    .collection::<Document>(collection)
                    .count_documents(doc! {})
                    .await
                    .unwrap(),
                u64::from(issue_first),
                "{collection}"
            );
        }
        if let Some(issued) = issued {
            assert_eq!(
                store.grant(&issued.id).await.unwrap().unwrap().view(),
                issued
            );
        }
        f.client.database(&f.database).drop().await.unwrap();
    }
}
