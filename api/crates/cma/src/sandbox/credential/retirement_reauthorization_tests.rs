use super::*;
use layer2_core::sandbox::{RetirementAuthorityVersion, RetirementCredentialRecovery};

struct ScopedTokens(Mutex<Option<RetirementAuthorityVersion>>);

#[async_trait::async_trait]
impl OwnerTokenSource for ScopedTokens {
    async fn token_for(&self, _: &SubjectId) -> Result<Option<SecretValue>, String> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .map(|_| SecretValue::new("fixture-owner-authority".into())))
    }

    async fn retirement_authority_version(
        &self,
        _: &Sandbox,
        _: &SecretValue,
    ) -> Result<Option<RetirementAuthorityVersion>, CredentialError> {
        Ok(*self.0.lock().unwrap())
    }
}

#[tokio::test]
async fn missing_owner_token_uses_exact_issuance_scope_on_revoke() {
    let (base_url, provider, server) = provider().await;
    let original = RetirementAuthorityVersion {
        grant: [1; 16],
        scope: [2; 32],
        granted_at_ms: 1,
    };
    let tokens = Arc::new(ScopedTokens(Mutex::new(Some(original))));
    let custody = Arc::new(MemoryCustody::default());
    let controller = ManagedSandboxCredentials::new(
        Arc::new(KeyIssuer::new(reqwest::Client::new(), base_url, "test")),
        tokens.clone(),
        custody.clone(),
    )
    .with_retirement_reauthorization(true);
    let mut row = sandbox();
    let issued = protected_issue(&controller, &row, 1).await.unwrap();
    assert_eq!(
        custody
            .intent
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .owner_authority,
        Some(original)
    );
    row.status = SandboxStatus::Retiring;
    row.work.step = SandboxWorkStep::RevokeRetirementCredentials;
    row.credential.key_generation = 1;
    let id = &issued.authority.provider_key_id;
    *tokens.0.lock().unwrap() = None;
    assert_eq!(
        controller.revoke(&row, id).await,
        Err(CredentialError::OwnerReauthorization {
            previous: original,
            missing: true,
        })
    );
    assert!(matches!(
        controller.revoke(&row, "different-key").await,
        Err(CredentialError::Security(_))
    ));
    let admitted = RetirementAuthorityVersion {
        grant: [3; 16],
        granted_at_ms: 4,
        ..original
    };
    row.work.retirement_credential_recovery = Some(RetirementCredentialRecovery {
        expected_scope: original.scope,
        newer_than_ms: original.granted_at_ms,
        admitted: Some(admitted),
    });
    *tokens.0.lock().unwrap() = Some(RetirementAuthorityVersion {
        scope: [5; 32],
        ..admitted
    });
    assert_eq!(
        controller.revoke(&row, id).await,
        Err(CredentialError::OwnerReauthorization {
            previous: admitted,
            missing: false,
        })
    );
    assert!(provider.revoked.lock().unwrap().is_empty());
    *tokens.0.lock().unwrap() = Some(admitted);
    controller.revoke(&row, id).await.unwrap();
    assert_eq!(*provider.revoked.lock().unwrap(), [id.clone()]);
    server.abort();
}
