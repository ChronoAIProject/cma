//! Independent revoke-only reconciliation of known late initial mint results.
use std::sync::Arc;
use std::time::Duration;

use crate::auth::{keys::KeyIssuer, persistence::SecretValue, AuthState};
use crate::sandbox::{CredentialCustodyError, IssuanceIdentity, RetirementOwnerAuthority};
use crate::store::MongoSandboxCredentialStore;
use tokio::{sync::watch, task::JoinHandle};

#[async_trait::async_trait]
pub(crate) trait RetirementOwnerTokenSource: Send + Sync {
    async fn exact_token(&self, authority: &RetirementOwnerAuthority) -> Option<SecretValue>;
}

#[async_trait::async_trait]
impl RetirementOwnerTokenSource for AuthState {
    async fn exact_token(&self, authority: &RetirementOwnerAuthority) -> Option<SecretValue> {
        match authority {
            RetirementOwnerAuthority::Personal {
                owner,
                authorization,
            } => {
                self.retained_personal_owner_authority(owner, authorization)
                    .await
            }
            RetirementOwnerAuthority::OwnerApi {
                owner,
                authorization,
            } => {
                self.retained_owner_service_authority(owner, authorization)
                    .await
            }
        }
        .ok()
        .map(|(_, token)| token)
    }
}

pub(crate) struct RetirementRevocationWorker {
    custody: Arc<MongoSandboxCredentialStore>,
    issuer: Arc<KeyIssuer>,
    owner_tokens: Arc<dyn RetirementOwnerTokenSource>,
    holder: String,
}

impl RetirementRevocationWorker {
    pub(crate) fn new(
        custody: Arc<MongoSandboxCredentialStore>,
        issuer: Arc<KeyIssuer>,
        owner_tokens: Arc<dyn RetirementOwnerTokenSource>,
        holder: String,
    ) -> Self {
        Self {
            custody,
            issuer,
            owner_tokens,
            holder,
        }
    }

    pub(crate) fn spawn(self, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                    _ = interval.tick() => {
                        if *shutdown.borrow() { break; }
                        if self.run_one().await.is_err() {
                            tracing::warn!("retirement credential revocation remains unresolved");
                        }
                    }
                }
            }
        })
    }

    pub(crate) async fn run_one(&self) -> Result<bool, CredentialCustodyError> {
        let Some(claim) = self
            .custody
            .claim_retirement_revocation(&self.holder)
            .await?
        else {
            return Ok(false);
        };
        let provider = self.issuer.provider_identity();
        let settled = async {
            if !matches!(&claim.issuance.identity, IssuanceIdentity::Prepared { provider_identity, .. } if provider_identity == &provider) {
                return Ok(false);
            }
            let Some(admission) = self.custody.admit_retirement_revocation(&claim, &provider).await? else {
                return Ok(false);
            };
            if !self.custody.retirement_revocation_admission_current(&admission).await? {
                return Ok(false);
            }
            let Some(token) = self.owner_tokens.exact_token(&claim.owner_authority).await else {
                return Ok(false);
            };
            if !self.custody.retirement_revocation_admission_current(&admission).await? {
                return Ok(false);
            }
            let evidence = match self.issuer.revoke_retirement_key(&claim.provider_authority.provider_key_id, token.expose()).await {
                Ok(evidence) => evidence,
                Err(_) => return Ok(false),
            };
            self.custody.record_retirement_key_revocation(&admission, &evidence).await
        }.await;
        if !matches!(settled, Ok(true)) {
            self.custody.defer_retirement_revocation(&claim).await?;
        }
        settled.map(|_| true)
    }
}
