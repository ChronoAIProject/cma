use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn promotion_preserves_a_possibly_dispatched_legacy_mint() {
    let mut fixture = Fixture::new_mode("mint_legacy_promotion", false).await;
    assert!(fixture.claim.work.authority_protocol.is_none());
    let enabled = peer(&fixture.name).await.with_renewable_work(true);
    Kind::Sandbox
        .set(
            &enabled,
            doc! {"work_lease_expires_at":at(-1),
            "record.work.lease.expires_at":at(-1)},
        )
        .await;
    fixture.claim = enabled
        .claim_sandbox_work(&fixture.claim.id, "participating-successor", at(0), at(15))
        .await
        .unwrap()
        .unwrap();
    fixture.store = enabled;
    let checking = fixture.issue(1);
    let request = fixture.peer.next().await;
    assert_eq!(request.method, axum::http::Method::GET);
    assert_eq!(request.uri.path(), "/api/v1/api-keys");
    request.json(json!([]));
    assert!(matches!(
        checking.await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    for generation in [2, 8, 1] {
        assert!(matches!(
            fixture.issue(generation).await.unwrap().unwrap(),
            Err(CredentialError::Uncertain)
        ));
    }
    let intent = fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        intent.identity,
        crate::sandbox::IssuanceIdentity::LegacyUnobserved
    );
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 0);
    assert_eq!(fixture.peer.count(), 1);
    // Removing the aggregate cannot erase the obligation or the rollback floor.
    fixture
        .store
        .sandboxes
        .delete_one(doc! {"_id":fixture.claim.id.as_str()})
        .await
        .unwrap();
    assert!(fixture
        .custody
        .requires_no_resend(&fixture.claim)
        .await
        .unwrap());
    assert_eq!(
        fixture.custody.issuance(&fixture.claim).await.unwrap(),
        Some(intent)
    );
    let floor = fixture
        .client
        .database(&fixture.name)
        .collection::<Document>("sandbox_credentials")
        .find_one(doc! {"_id":"ordinary-work-writer-floor-v1"})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(floor.get_i64("document_schema_version").unwrap(), 3);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn malformed_or_missing_intent_refuses_another_post() {
    let mut fixture = Fixture::new("mint_missing_custody").await;
    let original = fixture.issue(1);
    let accepted = fixture.prepare().await;
    accepted.stall_body();
    original.abort();
    assert!(original.await.unwrap_err().is_cancelled());
    fixture.takeover().await;
    let rows = fixture
        .client
        .database(&fixture.name)
        .collection::<Document>("sandbox_credential_issuance");
    let retained = rows
        .find_one(doc! {"_id":fixture.claim.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    rows.update_one(
        doc! {"_id":fixture.claim.id.as_str()},
        doc! {"$set":{"record.identity.request_digest":"malformed"}},
    )
    .await
    .unwrap();
    assert!(matches!(
        fixture.issue(1).await.unwrap().unwrap(),
        Err(CredentialError::Security(_))
    ));
    assert_eq!(fixture.peer.count(), 3);
    rows.replace_one(doc! {"_id":fixture.claim.id.as_str()}, retained)
        .await
        .unwrap();
    rows.delete_one(doc! {"_id":fixture.claim.id.as_str()})
        .await
        .unwrap();
    let retry = fixture.issue(1);
    super::acknowledgements::prepare_only(&mut fixture).await;
    assert!(matches!(
        retry.await.unwrap().unwrap(),
        Err(CredentialError::Unavailable(_))
    ));
    assert_eq!(fixture.peer.count_route("POST", "/api/v1/api-keys"), 1);
    assert!(fixture
        .custody
        .issuance(&fixture.claim)
        .await
        .unwrap()
        .is_none());
    fixture.cleanup().await;
}
