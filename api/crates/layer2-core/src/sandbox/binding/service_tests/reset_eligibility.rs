use super::*;

#[tokio::test]
async fn binding_service_requires_both_capabilities_and_owner_isolation() {
    let f = fixture();
    let value = profile("Original instruction");
    let digest = format!("sha256:{}", content_digest(&value).unwrap());
    for capability in [
        Capability::ManageGatewayRecords,
        Capability::ManageProviderBindings,
    ] {
        let caller = Caller::new(f.caller.subject.clone(), Authorization::new([capability]));
        assert!(matches!(
            f.service
                .put(&caller, "binding-one", value.clone(), &digest, &f.key)
                .await,
            Err(BindingError::Forbidden)
        ));
    }
    f.put("binding-one", "Original instruction").await;
    let other = Caller::new(
        SubjectId::new("another-manager"),
        f.caller.authorization.clone(),
    );
    assert!(matches!(
        f.service.read(&other, "binding-one").await,
        Err(BindingError::NotFound)
    ));
    f.service.retire(&f.caller, "binding-one").await.unwrap();
    for _ in 0..3 {
        let value = f.binding().await;
        f.service.reconcile_binding(&value).await.unwrap();
    }
    assert!(f.custody.keys.lock().unwrap().is_empty());
    assert_eq!(f.binding().await.work, BindingWork::Idle);
    assert!(matches!(
        f.service
            .put(&f.caller, "binding-one", value, &digest, &f.key)
            .await,
        Err(BindingError::Conflict)
    ));
}
