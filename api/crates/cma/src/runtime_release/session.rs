use std::sync::Arc;

use layer3_adapter::effect_authority::{
    EffectAuthority, EffectIntent, EffectPermit, ObservedOutcome,
};
use layer3_adapter::{AdapterError, Result};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::release_authority::{self as authority, AuthorityStore, WriterIdentity};

pub(crate) struct Session {
    pub(crate) store: Arc<AuthorityStore>,
    pub(crate) writer: WriterIdentity,
}

impl Session {
    pub(crate) async fn current(&self) -> std::result::Result<authority::ReleaseHead, String> {
        let head = self
            .store
            .load(&self.writer.deployment)
            .await
            .map_err(|error| error.to_string())?
            .ok_or("runtime release head disappeared")?;
        if head.phase != authority::Phase::Active || head.writer.as_ref() != Some(&self.writer) {
            return Err("runtime release admission is frozen or selected another process".into());
        }
        Ok(head)
    }
}

#[async_trait::async_trait]
impl EffectAuthority for Session {
    async fn admit(&self, intent: &EffectIntent) -> Result<Box<dyn EffectPermit>> {
        intent.validate()?;
        let intent = authority::EffectIntent {
            effect_id: uuid::Uuid::new_v4().to_string(),
            resource_identity_sha256: format!("{:x}", Sha256::digest(intent.resource.as_bytes())),
            request_sha256: intent.request_sha256.clone(),
            kind: intent.operation.clone(),
        };
        match self
            .store
            .admit_effect(&self.writer, &intent)
            .await
            .map_err(unavailable)?
        {
            authority::EffectAdmission::New(permit) => Ok(Box::new(Permit {
                store: self.store.clone(),
                permit: Mutex::new(permit),
            })),
            authority::EffectAdmission::Existing(_) => Err(AdapterError::Rejected(
                "runtime effect already exists; reconcile original outcome before dispatch".into(),
            )),
        }
    }
}

struct Permit {
    store: Arc<AuthorityStore>,
    permit: Mutex<authority::EffectPermit>,
}

#[async_trait::async_trait]
impl EffectPermit for Permit {
    async fn record_outcome(&self, outcome: ObservedOutcome) -> Result<()> {
        let mut permit = self.permit.lock().await;
        // This receipt records what the wrapper observed, never receiver or
        // lifecycle terminality. The settlement verifier must acquire those.
        let receipt_sha256 =
            layer3_adapter::provider_request_digest(&(&permit.intent.request_sha256, outcome))?;
        let observed = authority::ObservedOutcome {
            receipt_sha256,
            receiver_incarnation_sha256: None,
        };
        *permit = self
            .store
            .record_effect_outcome(&permit, &observed)
            .await
            .map_err(unavailable)?;
        Ok(())
    }
}

fn unavailable(error: authority::AuthorityError) -> AdapterError {
    tracing::warn!(error = %error, "runtime release effect authority refused");
    AdapterError::Unavailable("runtime release effect authority is unavailable or frozen".into())
}
