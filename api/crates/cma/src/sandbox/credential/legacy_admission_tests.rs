use super::*;

#[tokio::test]
async fn fresh_legacy_issuance_refuses_before_owner_authority_or_provider_requests() {
    let (endpoint, provider, server) = provider().await;
    let custody = Arc::new(MemoryCustody::default());
    let tokens = Arc::new(Tokens::default());
    for _ in 0..2 {
        let controller = ManagedSandboxCredentials::new(
            Arc::new(KeyIssuer::new(reqwest::Client::new(), &endpoint, "test")),
            tokens.clone(),
            custody.clone(),
        );
        let result = controller.issue(&sandbox(), 1, SystemTime::now()).await;
        assert_eq!(
            provider.mints.load(Ordering::SeqCst),
            0,
            "unprotected mint must never dispatch"
        );
        assert_eq!(tokens.calls.load(Ordering::SeqCst), 0);
        assert!(matches!(result, Err(CredentialError::Unavailable(reason))
            if reason == "managed credential issuance requires protected work authority"));
        assert!(custody.values.lock().unwrap().is_empty());
        assert!(custody.intent.lock().unwrap().is_none());
    }
    server.abort();
}

#[tokio::test]
async fn retained_legacy_generation_survives_restart_and_cleanup_without_reminting() {
    let (endpoint, provider, server) = provider().await;
    let custody = Arc::new(MemoryCustody::default());
    let tokens = Arc::new(Tokens::default());
    let original = material(sandbox().owner, "retained-provider-key");
    custody.seed_retained(original.clone()).unwrap();
    for _ in 0..2 {
        let controller = ManagedSandboxCredentials::new(
            Arc::new(KeyIssuer::new(reqwest::Client::new(), &endpoint, "test")),
            tokens.clone(),
            custody.clone(),
        );
        let recovered = controller
            .issue(&sandbox(), 1, SystemTime::now())
            .await
            .unwrap();
        assert_eq!(recovered.authority, original.authority);
        assert_eq!(recovered.value.expose(), original.value.expose());
        assert!(matches!(
            controller.issue(&sandbox(), 2, SystemTime::now()).await,
            Err(CredentialError::Unavailable(_))
        ));
        for wrong_source in [false, true] {
            let mut wrong = sandbox();
            if wrong_source {
                wrong.credential.source = layer2_core::sandbox::SandboxCredentialSource::Provided {
                    key_sha256: "a".repeat(64),
                };
            } else {
                wrong.owner = SubjectId::new("different-owner");
            }
            assert!(matches!(
                controller.issue(&wrong, 1, SystemTime::now()).await,
                Err(CredentialError::Security(_))
            ));
        }
    }
    assert_eq!(tokens.calls.load(Ordering::SeqCst), 0);
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(reqwest::Client::new(), endpoint, "test")),
        tokens,
        custody.clone(),
    );
    assert_eq!(controller.custodied(&sandbox().id).await.unwrap().len(), 1);
    controller
        .revoke(&sandbox(), &original.authority.provider_key_id)
        .await
        .unwrap();
    controller.forget(&sandbox().id, 1).await.unwrap();
    assert!(controller
        .custodied(&sandbox().id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(provider.mints.load(Ordering::SeqCst), 0);
    assert_eq!(*provider.revoked.lock().unwrap(), ["retained-provider-key"]);
    server.abort();
}

#[tokio::test]
async fn retained_revocation_debt_is_retried_before_recovering_legacy_material() {
    let (endpoint, provider, server) = provider().await;
    let custody = Arc::new(MemoryCustody::default());
    let original = material(sandbox().owner, "retained-winner");
    custody.seed_retained(original.clone()).unwrap();
    custody
        .queue_revocation(CredentialRevocation {
            sandbox_id: sandbox().id,
            owner: sandbox().owner,
            provider_key_id: "historical-loser".into(),
        })
        .await
        .unwrap();
    provider.revoke_failures.store(1, Ordering::SeqCst);
    for attempt in 0..2 {
        let controller = ManagedSandboxCredentials::new(
            Arc::new(KeyIssuer::new(reqwest::Client::new(), &endpoint, "test")),
            Arc::new(Tokens::default()),
            custody.clone(),
        );
        let result = controller.issue(&sandbox(), 1, SystemTime::now()).await;
        if attempt == 0 {
            assert!(matches!(result, Err(CredentialError::Unavailable(_))));
            assert_eq!(custody.revocations(&sandbox().id).await.unwrap().len(), 1);
        } else {
            assert_eq!(result.unwrap().authority, original.authority);
            assert!(custody.revocations(&sandbox().id).await.unwrap().is_empty());
        }
    }
    assert_eq!(
        *provider.revoked.lock().unwrap(),
        ["historical-loser", "historical-loser"]
    );
    assert_eq!(provider.mints.load(Ordering::SeqCst), 0);
    server.abort();
}
