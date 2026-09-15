use super::super::{caller, change, now, proposal, renew_retained};
use super::*;
use crate::bridge::custody::BridgeCredentialStore;
use layer2_core::{
    bridge::{
        allocation::{AllocationReservation, AllocationStore},
        AcceptedWorkPolicy, BridgeChange, BridgeStore,
    },
    sandbox::store::SandboxStore,
};

async fn disable(f: &mut RenewalFixture, accepted_work: AcceptedWorkPolicy, key: &str) {
    let bridge = f
        .original
        .bridges
        .bridge(&f.original.record.pin.bridge_id)
        .await
        .unwrap()
        .unwrap();
    f.original
        .svc
        .mutate(
            &caller(),
            None,
            change(
                &bridge.id,
                bridge.revision,
                key,
                BridgeChange::Disable { accepted_work },
            ),
        )
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn retained_ready_allocation_renews_after_reset_under_drain_and_holds_under_suspend() {
    let mut f = RenewalFixture::new().await;
    let original = f.original.record.clone();
    let (admission, authority, management) = proposal(
        &f.original.svc,
        &f.original.bridges,
        &original.pin.bridge_id,
        "12345",
        Some(&original),
    )
    .await;
    let replacement = f
        .original
        .allocations
        .reserve(
            AllocationReservation::reset(admission, authority, original.clone(), management, now())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(replacement.id, original.id);
    f.original.record = f
        .original
        .allocations
        .generation(&original.identity)
        .await
        .unwrap()
        .unwrap();
    let retained = f.original.record.clone();
    assert_eq!(
        retained.state,
        layer2_core::bridge::allocation::AllocationState::Retired
    );
    assert!(retained.receipt == original.receipt);
    disable(
        &mut f,
        AcceptedWorkPolicy::DrainOriginal,
        "renewal-drain-original",
    )
    .await;
    f.original.authority = renew_retained(&f.original.svc, &f.original.bridges, &retained)
        .await
        .unwrap();
    for _ in 0..3 {
        f.step().await.unwrap();
    }
    let renewed = f.snapshot().await;
    assert!(renewed.allocation == retained);
    assert_eq!(
        renewed
            .sandbox
            .credential
            .current
            .as_ref()
            .unwrap()
            .generation,
        2
    );
    assert_eq!(
        f.original
            .allocations
            .current(&replacement.identity)
            .await
            .unwrap()
            .unwrap()
            .id,
        replacement.id
    );
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    disable(
        &mut f,
        AcceptedWorkPolicy::SuspendOriginal,
        "renewal-suspend-original",
    )
    .await;
    assert!(
        renew_retained(&f.original.svc, &f.original.bridges, &retained)
            .await
            .is_err()
    );
    assert!(f.step().await.is_err());
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        f.original
            .resources
            .get_sandbox(&renewed.sandbox.id)
            .await
            .unwrap()
            .unwrap()
            .credential,
        renewed.sandbox.credential
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn exact_replaced_human_grant_refuses_renewal_mint_and_pending_delivery() {
    changed_grant_refuses(false).await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn missing_human_grant_refuses_renewal_mint_and_pending_delivery() {
    changed_grant_refuses(true).await;
}

async fn changed_grant_refuses(remove: bool) {
    for after_mint in [false, true] {
        let mut f = RenewalFixture::new().await;
        if after_mint {
            f.step().await.unwrap();
        }
        disable(
            &mut f,
            AcceptedWorkPolicy::DrainOriginal,
            "renewal-grant-disable",
        )
        .await;
        f.original.authority =
            renew_retained(&f.original.svc, &f.original.bridges, &f.original.record)
                .await
                .unwrap();
        let current = f.original.authority.bridge();
        let management = f
            .original
            .svc
            .management_lease(&caller(), None, &current.id, current.revision)
            .await
            .unwrap();
        let mut material = f
            .original
            .bridges
            .credentials
            .get(&f.original.record.config.connector.custody)
            .await
            .unwrap()
            .unwrap();
        let revision = material.metadata.revision;
        if remove {
            material.control_plane = None;
        } else {
            material
                .control_plane
                .as_mut()
                .unwrap()
                .incarnation
                .push_str("-replaced");
        }
        let before = f
            .original
            .resources
            .get_sandbox(&f.original.record.placement.target().sandbox_id)
            .await
            .unwrap()
            .unwrap();
        let requests = f.original.backend.requests().await.len();
        if remove {
            f.original.client.database(&f.original.database).collection::<Document>("bridge_credentials")
                    .update_one(doc! {"_id":f.original.record.config.connector.custody.as_str(),"metadata.revision":revision as i64},
                        doc! {"$unset":{"control_plane":""}}).await.unwrap();
            assert!(f
                .original
                .bridges
                .credentials
                .get(&f.original.record.config.connector.custody)
                .await
                .unwrap()
                .unwrap()
                .control_plane
                .is_none());
        } else {
            assert_eq!(
                f.original
                    .bridges
                    .credentials
                    .admit(&management, material)
                    .await
                    .unwrap()
                    .revision,
                revision
            );
        }
        assert!(f.step().await.is_err());
        assert_eq!(
            f.state.mint_count.load(Ordering::SeqCst),
            usize::from(after_mint)
        );
        assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 0);
        assert_eq!(f.original.backend.requests().await.len(), requests);
        assert_eq!(
            f.original
                .resources
                .get_sandbox(&before.id)
                .await
                .unwrap()
                .unwrap(),
            before
        );
        f.close().await;
    }
}
