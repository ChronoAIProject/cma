use super::*;

#[tokio::test]
async fn org_access_concealing_404_needs_current_admin_listing_and_exact_absence() {
    let f = Fixture::new(ResourceOwner::Organization {
        subject: SubjectId::new("fixture-org"),
    })
    .await;
    *f.state.status.lock().unwrap() = StatusCode::NOT_FOUND;
    *f.state.listing_status.lock().unwrap() = StatusCode::FORBIDDEN;
    assert!(matches!(
        f.revoke().await,
        Err(KeyIssuerError::Refused(403))
    ));
    // Authorized but conflicting detail/list evidence cannot prove deletion.
    *f.state.listing_status.lock().unwrap() = StatusCode::OK;
    assert!(matches!(
        f.revoke().await,
        Err(KeyIssuerError::UnexpectedAuthority)
    ));
    *f.state.listing_override.lock().unwrap() = Some(json!({"keys":[],"next_cursor":"incomplete"}));
    assert!(f.revoke().await.is_err());
    *f.state.listing_override.lock().unwrap() = Some(json!({"keys":[]}));
    f.revoke().await.unwrap();
    assert_eq!(f.state.deletes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn uncertain_delete_404_also_requires_current_owner_absence_before_completion() {
    let f = Fixture::new(ResourceOwner::Organization {
        subject: SubjectId::new("fixture-org"),
    })
    .await;
    *f.state.delete_response.lock().unwrap() = StatusCode::NOT_FOUND;
    *f.state.listing_status.lock().unwrap() = StatusCode::FORBIDDEN;
    assert!(matches!(
        f.revoke().await,
        Err(KeyIssuerError::Refused(403))
    ));
    assert_eq!(f.state.deletes.load(Ordering::SeqCst), 1);
    *f.state.listing_status.lock().unwrap() = StatusCode::OK;
    f.revoke().await.unwrap();
    assert_eq!(f.state.deletes.load(Ordering::SeqCst), 1);
}
