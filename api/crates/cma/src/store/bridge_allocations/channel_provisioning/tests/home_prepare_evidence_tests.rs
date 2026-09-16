//! A later success never turns ambiguous prepare history into removal authority.
use super::*;
use crate::sandbox::channel_provisioning::home_prepare_evidence::{
    HomePhysicalCleanupCapability, HomePhysicalProof, HomePrepareReconciliation,
};
use layer2_core::bridge::allocation::{AllocationMutation, AllocationStore};

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_prepare_unknown_history_survives_later_generation_and_progression() {
    let (mut f, homes) = fixture(true).await;
    f.step().await.unwrap();
    let original = provenance(&row(&f).await).unwrap();
    let debt = prepare_evidence(&row(&f).await).unwrap();
    assert!(homes.requests.lock().await.is_empty());
    assert_eq!(
        debt.generation_history,
        HomePrepareReconciliation::Unresolved
    );
    assert_eq!(debt.provider_incarnation, HomePhysicalProof::Unproven);
    assert_eq!(debt.backing_storage_absence, HomePhysicalProof::Unproven);
    assert_eq!(
        debt.physical_cleanup,
        HomePhysicalCleanupCapability::Unavailable
    );
    assert!(matches!(f.step().await, Err(AllocationError::Unavailable)));
    assert_eq!(prepare_evidence(&row(&f).await), Some(debt.clone()));
    // The first effect created a generation but lost its response. Model a
    // later matching generation at the same request identity. The first bound
    // response cannot prove what happened to the earlier physical storage.
    homes
        .stored
        .lock()
        .await
        .get_mut(&original.request.volume_id)
        .unwrap()
        .receipt
        .storage_generation = "later-observed-generation".into();
    f.release_current_claim().await;
    reopen(&mut f, homes.clone());
    f.step().await.unwrap();
    let observed = provenance(&row(&f).await).unwrap();
    assert_eq!(
        observed.first_receipt.as_ref().unwrap().storage_generation,
        "later-observed-generation"
    );
    assert_eq!(prepare_evidence(&row(&f).await), Some(debt.clone()));
    assert_eq!(*homes.requests.lock().await, vec![original.request; 2]);
    let next = f.children().await;
    assert_eq!(next.home_prepare_evidence, Some(debt));
    assert_eq!(next.home_provenance, Some(observed));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_prepare_records_unknown_legacy_dispatch_before_another_prepare() {
    let (mut f, homes) = fixture(false).await;
    f.step().await.unwrap();
    let saved = row(&f).await;
    let debt = prepare_evidence(&saved).unwrap();
    // This models an accepted provenance22 row, before dispatch evidence existed.
    f.client
        .database(&f.database)
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":f.record.placement.target().sandbox_id.as_str()},
            doc! {"$unset":{"channel_home_prepare_evidence":""}},
        )
        .await
        .unwrap();
    f.step().await.unwrap();
    assert!(homes.requests.lock().await.is_empty());
    assert_eq!(prepare_evidence(&row(&f).await), Some(debt.clone()));
    f.step().await.unwrap();
    assert_eq!(homes.requests.lock().await.len(), 1);
    assert_eq!(prepare_evidence(&row(&f).await), Some(debt));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_home_prepare_store_refuses_to_erase_or_replace_dispatch_debt() {
    let (mut f, homes) = fixture(false).await;
    f.step().await.unwrap();
    let debt = prepare_evidence(&row(&f).await).unwrap();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    f.record = f
        .allocations
        .mutate(
            AllocationMutation::claim(
                f.record.clone(),
                f.authority.clone(),
                "prepare-debt-writer",
                at,
                60_000,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let snapshot = f
        .allocations
        .begin_channel_provisioning(&f.authority, &f.lease, &f.record, &f.settings)
        .await
        .unwrap();
    let mut changed = debt.clone();
    changed.original_provenance_sha256 = "f".repeat(64);
    for proposed in [None, Some(changed)] {
        let outcome = f
            .allocations
            .commit_channel_home_provisioning(
                &f.authority,
                &f.lease,
                &snapshot,
                snapshot.sandbox.clone(),
                ChannelProvisioningPhase::Home,
                snapshot.home_provenance.clone(),
                proposed,
            )
            .await;
        assert!(matches!(outcome, Err(AllocationError::IdentityMismatch)));
        assert_eq!(prepare_evidence(&row(&f).await), Some(debt.clone()));
        assert!(homes.requests.lock().await.is_empty());
    }
    f.release_current_claim().await;
    f.step().await.unwrap();
    assert_eq!(homes.requests.lock().await.len(), 1);
    assert_eq!(prepare_evidence(&row(&f).await), Some(debt));
    f.close().await;
}
