//! Hold a completed flight at its existing release boundary, before consumption.
use super::*;

#[tokio::test]
async fn equivalent_live_success_preserves_receipt_before_bearer_consumption() {
    let f = Fixture::new().await;
    let gate = f.block().await;
    let first = f.verify("fixture-access");
    f.arrived(1).await;
    let entries = f.auth.bearer_verifications.entries.lock().await;
    let key = SecretDigest::identify("independent-bearer-verification", "fixture-access");
    let flight = entries.get(&key).unwrap().upgrade().unwrap();
    gate.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !flight.initialized() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let receipt = flight.get().unwrap().as_ref().unwrap().clone();
    *f.control.gate.write().await = None;
    // Start an independent live verification while A waits to release its
    // completed flight. No completed result is borrowed by this caller.
    let later = crate::auth::identity_retry::verify(
        &f.auth,
        Some(&f.metadata),
        "fixture-access",
        None,
        None,
        crate::auth::identity_retry::DEADLINE,
    )
    .await
    .unwrap();
    assert!(!Arc::ptr_eq(&receipt, &later));
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
    drop(entries);
    assert!(first.await.unwrap().is_ok());
    assert!(crate::auth::identity_lifetime::current(&f.auth, &receipt));
    drop(receipt);
    drop(later);
    drop(flight);
    assert_eq!(f.auth.identity_store.active_receipts(), 0);
}

async fn live(f: &Fixture, expiry: Option<std::time::SystemTime>) -> Arc<VerifiedSubject> {
    crate::auth::identity_retry::verify(
        &f.auth,
        Some(&f.metadata),
        "fixture-access",
        expiry,
        None,
        crate::auth::identity_retry::DEADLINE,
    )
    .await
    .unwrap()
}

fn current(f: &Fixture, evidence: &VerifiedSubject) -> bool {
    crate::auth::identity_lifetime::current(&f.auth, evidence)
}

#[tokio::test]
async fn profile_only_changes_keep_each_receipt_and_cannot_grant_permissions() {
    let f = Fixture::new().await;
    let first = f.verify("fixture-access").await.unwrap().unwrap();
    *f.control.userinfo_body.write().await = Some(json!({
        "sub":"fixture-owner", "name":"Display change", "permissions":["cma:*"]
    }));
    let second = f.verify("fixture-access").await.unwrap().unwrap();
    assert!(current(&f, &first) && current(&f, &second));
    assert_eq!(first.profile.display_name, None);
    assert_eq!(
        second.profile.display_name.as_deref(),
        Some("Display change")
    );
    assert!(first.permissions == second.permissions);
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(f.control.userinfo_calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn caller_expiry_does_not_rotate_or_renew_an_equivalent_receipt() {
    use std::time::SystemTime;
    let f = Fixture::new().await;
    let earlier = live(&f, Some(SystemTime::now() + Duration::from_secs(60))).await;
    let later = live(&f, Some(SystemTime::now() + Duration::from_secs(600))).await;
    assert!(current(&f, &earlier) && current(&f, &later));
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(61)).await;
    assert!(!current(&f, &earlier));
    assert!(current(&f, &later));
}

#[tokio::test]
async fn expired_caller_evidence_cannot_publish_changed_authority() {
    let f = Fixture::new().await;
    let first = live(&f, None).await;
    *f.control.introspection_body.write().await = Some(json!({
        "active":true,"sub":"fixture-owner","permissions":[]
    }));
    let result = crate::auth::identity_retry::verify(
        &f.auth,
        Some(&f.metadata),
        "fixture-access",
        Some(std::time::SystemTime::UNIX_EPOCH),
        None,
        crate::auth::identity_retry::DEADLINE,
    )
    .await;
    assert!(matches!(
        result,
        Err(ProviderVerificationError::Rejected(_))
    ));
    assert!(current(&f, &first));
}

#[tokio::test]
async fn provider_expiry_changes_conservatively_fence_previous_receipts() {
    use std::time::SystemTime;
    let f = Fixture::new().await;
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let mut previous = live(&f, None).await;
    for expiry in [
        Some(seconds + 600),
        Some(seconds + 300),
        Some(seconds + 900),
        None,
    ] {
        let mut body =
            json!({"active":true,"sub":"fixture-owner","permissions":["cma:sandbox:use"]});
        if let Some(expiry) = expiry {
            body["exp"] = json!(expiry);
        }
        *f.control.introspection_body.write().await = Some(body);
        let next = live(&f, None).await;
        assert!(!current(&f, &previous));
        assert!(current(&f, &next));
        previous = next;
    }
    *f.control.introspection_body.write().await = Some(json!({
        "active":true,"sub":"fixture-owner","permissions":["cma:sandbox:use"],"exp":1
    }));
    assert!(matches!(
        f.verify("fixture-access").await.unwrap(),
        Err(ProviderVerificationError::Rejected(_))
    ));
    assert!(!current(&f, &previous));
}

#[tokio::test]
async fn changed_provider_endpoint_fences_old_authority() {
    let f = Fixture::new().await;
    let first = live(&f, None).await;
    let mut metadata = f.metadata.clone();
    metadata
        .introspection_endpoint
        .as_mut()
        .unwrap()
        .push_str("?scope=changed");
    let second = crate::auth::identity_retry::verify(
        &f.auth,
        Some(&metadata),
        "fixture-access",
        None,
        None,
        crate::auth::identity_retry::DEADLINE,
    )
    .await
    .unwrap();
    assert!(!current(&f, &first));
    assert!(current(&f, &second));
    assert_eq!(f.control.introspection_calls.load(Ordering::SeqCst), 2);
}

async fn retained(f: &Fixture) -> Arc<VerifiedSubject> {
    crate::auth::identity_retry::verify(
        &f.auth,
        Some(&f.metadata),
        "fixture-access",
        None,
        Some("fixture-owner"),
        crate::auth::identity_retry::DEADLINE,
    )
    .await
    .unwrap()
}

/// #2709: a browser's cookie (retained-owner) verification overlaps an
/// independent verification of the same credential, as the organization
/// principal performs. Equivalent authority must not fence either one; the
/// later publisher was refused with `store_failed` before this was fixed.
#[tokio::test]
async fn verification_purpose_alone_never_fences_overlapping_equivalent_authority() {
    for independent_first in [true, false] {
        let f = Fixture::new().await;
        let earlier = retained(&f).await;
        let gate = f.block().await;
        let (auth, metadata) = (f.auth.clone(), f.metadata.clone());
        let held = tokio::spawn(async move {
            crate::auth::identity_retry::verify(
                &auth,
                Some(&metadata),
                "fixture-access",
                None,
                // The held call always has the other purpose from the overlap.
                independent_first.then_some("fixture-owner"),
                crate::auth::identity_retry::DEADLINE,
            )
            .await
        });
        f.arrived(1).await;
        *f.control.gate.write().await = None;
        let overlapping = if independent_first {
            live(&f, None).await
        } else {
            retained(&f).await
        };
        gate.release.add_permits(1);
        let held = held.await.unwrap().unwrap();
        for evidence in [&earlier, &overlapping, &held] {
            assert!(
                current(&f, evidence),
                "independent first: {independent_first}"
            );
        }
    }
}

#[tokio::test]
async fn changed_authority_from_either_purpose_still_fences_the_other() {
    for independent_changes in [true, false] {
        let f = Fixture::new().await;
        let first = if independent_changes {
            retained(&f).await
        } else {
            live(&f, None).await
        };
        *f.control.introspection_body.write().await = Some(json!({
            "active":true,"sub":"fixture-owner","permissions":[]
        }));
        let second = if independent_changes {
            live(&f, None).await
        } else {
            retained(&f).await
        };
        assert!(!current(&f, &first));
        assert!(current(&f, &second));
    }
}
