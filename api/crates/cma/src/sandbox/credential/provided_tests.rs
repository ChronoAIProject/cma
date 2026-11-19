use super::*;
use layer2_core::sandbox::binding::{BindingCredentialCustody, ProvidedCredential};
use layer2_core::sandbox::SandboxCredentialSource;

fn provided_key() -> ProvidedCredential {
    ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap()
}

#[tokio::test]
async fn supplied_key_survives_controller_restart_without_mint_or_provider_revocation() {
    let (base, provider, server) = provider().await;
    let custody = Arc::new(MemoryCustody::default());
    let tokens = Arc::new(Tokens::default());
    let issuer = Arc::new(KeyIssuer::new(reqwest::Client::new(), base, "test"));
    let first = ManagedSandboxCredentials::new(issuer.clone(), tokens.clone(), custody.clone());
    let key = provided_key();
    let mut value = sandbox();
    value.credential.source = SandboxCredentialSource::Provided {
        key_sha256: key.sha256(),
    };
    first
        .retain(&value.id, &value.owner, &value.credential.reaches, &key)
        .await
        .unwrap();
    first
        .retain(&value.id, &value.owner, &value.credential.reaches, &key)
        .await
        .unwrap();
    let second = ManagedSandboxCredentials::new(issuer, tokens.clone(), custody.clone());
    let material = second.issue(&value, 1, SystemTime::now()).await.unwrap();
    assert_eq!(material.value.expose(), key.expose());
    assert!(material.authority.expires_at.is_empty());
    assert!(!format!("{material:?} {key:?}").contains(key.expose()));
    assert!(second
        .retain(
            &value.id,
            &value.owner,
            &value.credential.reaches,
            &provided_key()
        )
        .await
        .is_err());
    assert!(second
        .retain(
            &value.id,
            &SubjectId::new("another-owner"),
            &value.credential.reaches,
            &key
        )
        .await
        .is_err());
    assert!(second
        .retain(
            &value.id,
            &value.owner,
            &[ServiceId::new("another-service")],
            &key
        )
        .await
        .is_err());
    second
        .revoke(&value, &material.authority.provider_key_id)
        .await
        .unwrap();
    second
        .forget_provider(&value.id, &material.authority.provider_key_id)
        .await
        .unwrap();
    assert!(custody.list(&value.id).await.unwrap().is_empty());
    assert_eq!(tokens.calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.mints.load(Ordering::SeqCst), 0);
    assert!(provider.revoked.lock().unwrap().is_empty());
    server.abort();
}

#[tokio::test]
async fn absent_or_mismatched_supplied_custody_never_falls_back_to_managed_issuance() {
    let (base, provider, server) = provider().await;
    let custody = Arc::new(MemoryCustody::default());
    let tokens = Arc::new(Tokens::default());
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(reqwest::Client::new(), base, "test")),
        tokens.clone(),
        custody.clone(),
    );
    let key = provided_key();
    let mut value = sandbox();
    value.credential.source = SandboxCredentialSource::Provided {
        key_sha256: key.sha256(),
    };
    assert!(matches!(
        controller.issue(&value, 1, SystemTime::now()).await,
        Err(CredentialError::Security(_))
    ));
    custody
        .seed_retained(material(value.owner.clone(), "managed-key"))
        .unwrap();
    assert!(matches!(
        controller.issue(&value, 1, SystemTime::now()).await,
        Err(CredentialError::Security(_))
    ));
    assert_eq!(tokens.calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.mints.load(Ordering::SeqCst), 0);
    assert!(provider.revoked.lock().unwrap().is_empty());
    server.abort();
}

#[test]
fn supplied_state_defaults_old_records_and_refuses_source_changes() {
    let mut value = sandbox();
    let old = serde_json::to_value(&value).unwrap();
    assert!(old["credential"].get("source").is_none());
    let decoded: Sandbox = serde_json::from_value(old).unwrap();
    assert_eq!(decoded, value);
    let before = value.clone();
    value.credential.source = SandboxCredentialSource::Provided {
        key_sha256: provided_key().sha256(),
    };
    value.validate().unwrap();
    assert!(before.validate_service_access_successor(&value).is_err());
    value.credential.source = SandboxCredentialSource::Provided {
        key_sha256: "invalid".into(),
    };
    assert!(value.validate().is_err());
}
