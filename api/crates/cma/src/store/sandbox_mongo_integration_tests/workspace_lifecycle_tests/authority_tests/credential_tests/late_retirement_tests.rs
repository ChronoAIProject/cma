use super::legacy_admission_tests::retained;
use super::*;

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn committed_retirement_marker_retains_late_authority_without_delivery() {
    let mut f = Fixture::new_with_runtime("retirement_late_result", true, true).await;
    let issuing = f.issue(1);
    let mint = f.prepare().await;
    let intent = f.custody.issuance(&f.claim).await.unwrap().unwrap();
    let (_worker, _) = retirement_fixture::begin(&f).await;
    let (worker, controller) = retirement_fixture::native_custody_worker(&f, true, true);
    assert!(worker
        .run_one(&f.claim.id, SystemTime::now())
        .await
        .unwrap());
    let independent = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let records = independent
        .database(&f.name)
        .collection::<Document>("sandbox_credential_issuance");
    let filter = doc! {"retirement_custody.state":"pending", "retirement_custody.issuance.operation_id":&intent.operation_id};
    let before = records.find_one(filter.clone()).await.unwrap().unwrap();
    assert!(!before
        .get_document("retirement_custody")
        .unwrap()
        .contains_key("late_provider_authority"));

    let secret = accepted(mint);
    assert!(matches!(
        issuing.await.unwrap().unwrap(),
        Err(CredentialError::Uncertain)
    ));
    let after = records.find_one(filter.clone()).await.unwrap().unwrap();
    let marker = after.get_document("retirement_custody").unwrap();
    let authority = marker.get_document("late_provider_authority").unwrap();
    assert_eq!(
        authority.get_str("provider_key_id").unwrap(),
        "fixture-provider-key"
    );
    assert_eq!(authority.get_i64("provider_state_version").unwrap(), 7);
    assert_eq!(
        after
            .get_document("record")
            .unwrap()
            .get_str("state")
            .unwrap(),
        "unknown"
    );
    assert!(!format!("{after:?}").contains(&secret));
    assert!(f.custody.list(&f.claim.id).await.unwrap().is_empty());

    let mut duplicate = retained(&f);
    duplicate.authority = mongodb::bson::deserialize_from_document(authority.clone()).unwrap();
    duplicate.value = SecretValue::new(secret);
    assert!(f
        .custody
        .retain_late_retirement_result(&f.claim, &intent, &duplicate)
        .await
        .unwrap());
    duplicate.authority.provider_key_id = "other-provider-key".into();
    assert!(f
        .custody
        .retain_late_retirement_result(&f.claim, &intent, &duplicate)
        .await
        .is_err());
    assert_eq!(records.find_one(filter).await.unwrap().unwrap(), after);
    let retiring = f.store.get_sandbox(&f.claim.id).await.unwrap().unwrap();
    assert_eq!(retiring.status, SandboxStatus::Retiring);
    assert!(matches!(
        controller.settle_revocations(&retiring).await,
        Err(CredentialError::Uncertain)
    ));
    assert_eq!(f.peer.count_route("POST", "/api/v1/api-keys"), 1);
    f.cleanup().await;
}
