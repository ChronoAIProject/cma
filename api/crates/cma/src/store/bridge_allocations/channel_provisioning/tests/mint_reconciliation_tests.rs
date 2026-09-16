use super::*;
use crate::store::bridge_allocations::channel_mint_reconciliation::ChannelMintOrphanReceipt;
use serde_json::json;

async fn receipt(f: &RenewalFixture) -> Option<ChannelMintOrphanReceipt> {
    f.original
        .client
        .database(&f.original.database)
        .collection::<Document>("channel_mint_orphans")
        .find_one(doc! {"sandbox_id":f.original.record.placement.target().sandbox_id.as_str()})
        .await
        .unwrap()
        .map(|row| {
            mongodb::bson::deserialize_from_bson(row.get("receipt").unwrap().clone()).unwrap()
        })
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn unknown_initial_mint_records_exact_orphan_and_revokes_without_new_post_or_custody() {
    let mut f = super::super::fixture::Fixture::new().await;
    for _ in 0..3 {
        f.step().await.unwrap();
    }
    f.server
        .state
        .mint_unavailable
        .store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    f.release_current_claim().await;
    assert!(f.step().await.is_err());
    let row = f
        .client
        .database(&f.database)
        .collection::<Document>("channel_mint_orphans")
        .find_one(doc! {"allocation_id":&f.record.id})
        .await
        .unwrap()
        .unwrap();
    let receipt: ChannelMintOrphanReceipt =
        mongodb::bson::deserialize_from_bson(row.get("receipt").unwrap().clone()).unwrap();
    assert!(receipt.revoked);
    assert_eq!(receipt.key.provider_key_id, "key-created");
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.server.state.revoke_count.load(Ordering::SeqCst), 1);
    assert!(f
        .custody
        .get(&f.record.placement.target().sandbox_id, 1)
        .await
        .unwrap()
        .is_none());
    f.release_current_claim().await;
    assert!(f.step().await.is_err());
    assert_eq!(f.server.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.server.state.revoke_count.load(Ordering::SeqCst), 1);
    let text = format!("{row:?}");
    assert!(!text.contains(crate::auth::keys::bridge::test_fixture::HUMAN_SECRET));
    assert!(!text.contains(crate::auth::keys::bridge::test_fixture::RUNTIME_SECRET));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn unknown_renewal_reconciles_lost_delete_keeps_current_runtime_key_and_terminal_mint_hold() {
    let f = RenewalFixture::new().await;
    let before = f.snapshot().await;
    f.state.mint_unknown.store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    expire_claim(&f).await;
    assert_eq!(
        f.original
            .allocations
            .due_channel_renewal_sandboxes(None, SystemTime::now())
            .await
            .unwrap(),
        vec![before.sandbox.id.to_string()]
    );
    f.state.revoke_unknown.store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    assert!(!receipt(&f).await.unwrap().revoked);
    assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 1);
    expire_claim(&f).await;
    assert!(f.step().await.is_err());
    let receipt = receipt(&f).await.unwrap();
    assert!(receipt.revoked);
    assert_eq!(receipt.key.provider_key_id, "key-renewed-1");
    let after = f.snapshot().await;
    assert_eq!(after.sandbox.credential, before.sandbox.credential);
    assert_eq!(after.sandbox.runtime, before.sandbox.runtime);
    assert!(after.allocation == before.allocation);
    assert_eq!(
        after.renewal.as_ref().unwrap().phase,
        RenewalPhase::MintStarted
    );
    assert!(f.state.keys.lock().unwrap().contains_key(
        &before
            .sandbox
            .credential
            .current
            .as_ref()
            .unwrap()
            .provider_key_id
    ));
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert!(f
        .original
        .custody
        .get(&before.sandbox.id, 2)
        .await
        .unwrap()
        .is_none());
    assert!(f
        .original
        .allocations
        .due_channel_renewal_sandboxes(None, f.due)
        .await
        .unwrap()
        .is_empty());
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn uncertain_renewal_empty_or_ambiguous_listing_never_becomes_second_mint_or_broad_revoke() {
    let f = RenewalFixture::new().await;
    f.state.mint_unknown.store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    let key = f
        .state
        .keys
        .lock()
        .unwrap()
        .get("key-renewed-1")
        .unwrap()
        .clone();
    let mut other = key.clone();
    other["id"] = json!("another-key");
    for listing in [
        json!({"keys":[]}),
        json!({"keys":[key.clone(),other]}),
        json!({"keys":[key],"next_cursor":"truncated"}),
    ] {
        expire_claim(&f).await;
        *f.state.list_override.lock().unwrap() = Some(listing);
        assert!(f.step().await.is_err());
        assert!(receipt(&f).await.is_none());
        assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 0);
        assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn late_orphan_revocation_audit_failure_keeps_observed_receipt_for_exact_get_recovery() {
    let f = RenewalFixture::new().await;
    f.state.mint_unknown.store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    let db = f.original.client.database(&f.original.database);
    db.run_command(doc! {"collMod":"channel_mint_orphan_audits","validator":{"action":"observed"}})
        .await
        .unwrap();
    expire_claim(&f).await;
    assert!(f.step().await.is_err());
    assert!(!receipt(&f).await.unwrap().revoked);
    assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 1);
    db.run_command(doc! {"collMod":"channel_mint_orphan_audits","validator":{}})
        .await
        .unwrap();
    expire_claim(&f).await;
    assert!(f.step().await.is_err());
    assert!(receipt(&f).await.unwrap().revoked);
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        db.collection::<Document>("channel_mint_orphan_audits")
            .count_documents(doc! {})
            .await
            .unwrap(),
        2
    );
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn legacy_unknown_mint_without_original_scope_evidence_stays_held() {
    let f = RenewalFixture::new().await;
    f.state.mint_unknown.store(true, Ordering::SeqCst);
    assert!(f.step().await.is_err());
    expire_claim(&f).await;
    f.original
        .client
        .database(&f.original.database)
        .collection::<Document>("sandboxes")
        .update_one(
            doc! {"_id":f.original.record.placement.target().sandbox_id.as_str()},
            doc! {"$unset":{"channel_renewal.mint_intent":""}},
        )
        .await
        .unwrap();
    assert!(f.step().await.is_err());
    assert!(receipt(&f).await.is_none());
    assert_eq!(f.state.mint_count.load(Ordering::SeqCst), 1);
    assert_eq!(f.state.revoke_count.load(Ordering::SeqCst), 0);
    assert!(f
        .original
        .allocations
        .due_channel_renewal_sandboxes(None, f.due)
        .await
        .unwrap()
        .is_empty());
    f.close().await;
}
