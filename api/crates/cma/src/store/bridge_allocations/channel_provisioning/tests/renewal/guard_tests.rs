use super::*;
use crate::sandbox::{
    channel_credentials::ChannelGuardedCredentials, CredentialError, CredentialMaterial,
    SandboxCredentialController,
};
use layer2_core::{sandbox::Sandbox, SandboxId};
use std::sync::Arc;

struct ForbiddenOwnerTokenLane;
#[async_trait::async_trait]
impl SandboxCredentialController for ForbiddenOwnerTokenLane {
    async fn issue(
        &self,
        _: &Sandbox,
        _: u64,
        _: SystemTime,
    ) -> Result<CredentialMaterial, CredentialError> {
        panic!("Channel mint entered the ordinary owner-token lane");
    }
    async fn revoke(&self, _: &Sandbox, _: &str) -> Result<(), CredentialError> {
        panic!("Channel revoke entered the ordinary owner-token lane");
    }
    async fn settle_revocations(&self, _: &Sandbox) -> Result<(), CredentialError> {
        panic!("Channel cleanup entered the ordinary owner-token lane");
    }
    async fn material(
        &self,
        _: &SandboxId,
        _: u64,
    ) -> Result<Option<CredentialMaterial>, CredentialError> {
        Ok(None)
    }
    async fn custodied(&self, _: &SandboxId) -> Result<Vec<CredentialMaterial>, CredentialError> {
        Ok(vec![])
    }
    async fn forget(&self, _: &SandboxId, _: u64) -> Result<(), CredentialError> {
        panic!("ordinary cleanup removed Channel custody");
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_lifecycle_has_no_owner_token_fallback_even_when_resource_sidecar_is_missing() {
    let f = RenewalFixture::new().await;
    let snapshot = f.snapshot().await;
    let guarded = ChannelGuardedCredentials::new(
        Arc::new(ForbiddenOwnerTokenLane),
        f.original.allocations.clone(),
    );
    for remove_sidecar in [false, true] {
        if remove_sidecar {
            f.original.client.database(&f.original.database).collection::<Document>("sandboxes")
                .update_one(doc! {"_id":snapshot.sandbox.id.as_str()},
                    doc! {"$unset":{"channel_creation":"","channel_provisioning":"","channel_renewal":""}}).await.unwrap();
        }
        assert!(matches!(
            guarded.issue(&snapshot.sandbox, 2, SystemTime::now()).await,
            Err(CredentialError::Unavailable(_))
        ));
        assert!(matches!(
            guarded.require_lifecycle_restore(&snapshot.sandbox).await,
            Err(CredentialError::Unavailable(_))
        ));
        assert!(matches!(
            guarded
                .revoke(
                    &snapshot.sandbox,
                    &snapshot
                        .sandbox
                        .credential
                        .current
                        .as_ref()
                        .unwrap()
                        .provider_key_id
                )
                .await,
            Err(CredentialError::Unavailable(_))
        ));
        assert!(matches!(
            guarded.settle_revocations(&snapshot.sandbox).await,
            Err(CredentialError::Unavailable(_))
        ));
        assert!(matches!(
            guarded.forget(&snapshot.sandbox.id, 1).await,
            Err(CredentialError::Unavailable(_))
        ));
    }
    f.close().await;
}
