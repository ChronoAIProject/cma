use super::*;
use crate::{bridge::custody::BridgeCredentialStore, sandbox::CredentialMaterial};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_stage_rejects_replaced_control_grant_even_when_selected_key_revision_is_unchanged()
{
    let mut f = Fixture::new().await;
    f.step().await.unwrap();
    let bridge = f
        .bridges
        .bridge(&f.record.identity.bridge_id)
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
                "disable-grant-test",
                BridgeChange::Disable {
                    accepted_work: AcceptedWorkPolicy::DrainOriginal,
                },
            ),
        )
        .await
        .unwrap();
    f.authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
    let current = f.authority.bridge();
    let management = f
        .svc
        .management_lease(&caller(), None, &current.id, current.revision)
        .await
        .unwrap();
    let mut material = f
        .bridges
        .credentials
        .get(&f.record.config.connector.custody)
        .await
        .unwrap()
        .unwrap();
    let revision = material.metadata.revision;
    material
        .control_plane
        .as_mut()
        .unwrap()
        .incarnation
        .push_str("-replaced");
    assert_eq!(
        f.bridges
            .credentials
            .admit(&management, material)
            .await
            .unwrap()
            .revision,
        revision
    );
    assert!(f.step().await.is_err());
    assert!(f.backend.requests().await.is_empty());
    let sandbox = f
        .resources
        .get_sandbox(&f.record.placement.target().sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert!(sandbox.runtime.is_none() && sandbox.credential.pending.is_none());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_minted_secret_custody_collision_rolls_back_pending_authority_and_keeps_unknown_hold(
) {
    let mut f = Fixture::new().await;
    for _ in 0..3 {
        f.step().await.unwrap();
    }
    let at = std::time::SystemTime::now();
    f.custody
        .insert(CredentialMaterial {
            sandbox_id: f.record.placement.target().sandbox_id.clone(),
            owner: caller().caller.subject,
            value: crate::auth::persistence::SecretValue::new("existing-fixture-key".into()),
            authority: layer2_core::sandbox::SandboxCredentialAuthority {
                generation: 1,
                provider_key_id: "existing-key".into(),
                provider_state_version: 1,
                issued_at: layer2_core::rfc3339_at(at),
                expires_at: layer2_core::rfc3339_at(at + std::time::Duration::from_secs(3600)),
                grant_fingerprint: "c".repeat(64),
            },
        })
        .await
        .unwrap();
    assert!(f.step().await.is_err());
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    let sandbox = f
        .resources
        .get_sandbox(&f.record.placement.target().sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox.credential.key_generation, 0);
    assert!(sandbox.credential.pending.is_none() && !sandbox.credential.delivered);
    let doc = f
        .client
        .database(&f.database)
        .collection::<mongodb::bson::Document>("sandboxes")
        .find_one(doc! {"_id":sandbox.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        doc.get_document("channel_provisioning")
            .unwrap()
            .get_str("phase")
            .unwrap(),
        "mint_started"
    );
    assert_eq!(
        f.custody
            .get(&sandbox.id, 1)
            .await
            .unwrap()
            .unwrap()
            .authority
            .provider_key_id,
        "existing-key"
    );
    f.release_current_claim().await;
    assert!(f.step().await.is_err());
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_pending_discovery_advances_past_corrupt_aggregate_before_independent_decode() {
    let f = Fixture::new().await;
    f.client
        .database(&f.database)
        .collection::<mongodb::bson::Document>("bridge_allocations")
        .insert_one(doc! {"_id":"000-corrupt","creates_sandbox":true,"record":{"state":"reserved"}})
        .await
        .unwrap();
    let discovered = f
        .allocations
        .pending_channel_allocations(None)
        .await
        .unwrap();
    assert_eq!(discovered.first().unwrap(), "000-corrupt");
    assert!(f
        .allocations
        .channel_allocation(&discovered[0])
        .await
        .is_err());
    let later = f
        .allocations
        .pending_channel_allocations(Some(&discovered[0]))
        .await
        .unwrap();
    assert_eq!(later, vec![f.record.id.clone()]);
    assert!(
        f.allocations
            .channel_allocation(&later[0])
            .await
            .unwrap()
            .unwrap()
            == f.record
    );
    f.close().await;
}
