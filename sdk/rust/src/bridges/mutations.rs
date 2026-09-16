use super::*;
use crate::{Error, IdempotencyKey, Result};
use reqwest::{Method, StatusCode};
use serde::Serialize;

pub(super) fn management(key: &IdempotencyKey) -> Result<()> {
    if key.as_str().len() > 200 {
        return Err(Error::validation(
            "bridge idempotency key exceeds 200 bytes",
        ));
    }
    Ok(())
}
fn mutation_revision(value: u64) -> Result<()> {
    validation::revision(value)?;
    if value == i64::MAX as u64 {
        return Err(Error::validation(
            "bridge revision cannot advance beyond its bound",
        ));
    }
    Ok(())
}
#[derive(Serialize)]
struct Revision {
    expected_revision: u64,
}
#[derive(Serialize)]
struct Revoke {
    expected_revision: u64,
    expected_custody_revision: u64,
}

impl Bridges<'_> {
    /// Create a disabled draft with a stable idempotency key and explicit actual owner.
    pub async fn create(
        &self,
        body: &CreateBridgeRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgeReceipt> {
        management(key)?;
        if !validation::owner(&body.owner) {
            return Err(Error::validation("invalid bridge owner"));
        }
        body.config.check(true).map_err(Error::validation)?;
        let receipt: BridgeReceipt = self
            .client
            .command(
                Method::POST,
                &["bridges"],
                Some(body),
                key,
                StatusCode::CREATED,
            )
            .await?;
        receipt.check()?;
        if receipt.revision != 1
            || receipt.config_revision != 1
            || receipt.command != "create"
            || receipt.owner != body.owner
        {
            return Err(Error::protocol(
                "bridge creation receipt does not match its owner or initial revision",
            ));
        }
        Ok(receipt)
    }
    /// Replace configuration under an exact revision; existing target changes need explicit old-work policy.
    pub async fn update(
        &self,
        id: &str,
        body: &UpdateBridgeRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgeReceipt> {
        validation::id(id)?;
        mutation_revision(body.expected_revision)?;
        management(key)?;
        body.config.check(true).map_err(Error::validation)?;
        let receipt: BridgeReceipt = self
            .client
            .command(
                Method::PUT,
                &["bridges", id, "configuration"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await?;
        receipt.matches(id, body.expected_revision, "update")?;
        Ok(receipt)
    }
    /// Apply one explicit enable/disable/retire/emergency control.
    pub async fn control(
        &self,
        id: &str,
        body: &ControlBridgeRequest,
        key: &IdempotencyKey,
    ) -> Result<BridgeReceipt> {
        validation::id(id)?;
        mutation_revision(body.expected_revision)?;
        management(key)?;
        let receipt: BridgeReceipt = self
            .client
            .command(
                Method::POST,
                &["bridges", id, "commands"],
                Some(body),
                key,
                StatusCode::OK,
            )
            .await?;
        receipt.matches(id, body.expected_revision, body.command.kind())?;
        Ok(receipt)
    }
    /// Validate current provider/custody/target facts without enabling ingress.
    pub async fn validate(&self, id: &str, expected_revision: u64) -> Result<BridgeValidation> {
        validation::id(id)?;
        validation::revision(expected_revision)?;
        let value: BridgeValidation = self
            .client
            .operation(
                Method::POST,
                &["bridges", id, "validate"],
                Some(&Revision { expected_revision }),
                StatusCode::OK,
            )
            .await?;
        value.check(id, expected_revision)?;
        Ok(value)
    }
    /// Import the exact selected key into control-plane custody. The SDK borrows
    /// the secret for this request and never stores it in Client or Bridges.
    pub async fn import_key(
        &self,
        id: &str,
        body: &ImportBridgeKeyRequest<'_>,
    ) -> Result<BridgeCredentialStatus> {
        self.import_key_authority(id, body, None).await
    }
    /// Explicitly retain this exact human OAuth sign-in for the selected Bridge
    /// and owner for 30 days. Browser logout does not end this grant; revoking
    /// the imported key does. Each operation rechecks account and org admin access.
    pub async fn import_key_with_control_plane_authority(
        &self,
        id: &str,
        body: &ImportBridgeKeyRequest<'_>,
    ) -> Result<BridgeCredentialStatus> {
        self.import_key_authority(id, body, Some(true)).await
    }
    async fn import_key_authority(
        &self,
        id: &str,
        body: &ImportBridgeKeyRequest<'_>,
        retain_control_plane_authority: Option<bool>,
    ) -> Result<BridgeCredentialStatus> {
        #[derive(Serialize)]
        struct Import<'a, 'b> {
            #[serde(flatten)]
            body: &'a ImportBridgeKeyRequest<'b>,
            #[serde(skip_serializing_if = "Option::is_none")]
            retain_control_plane_authority: Option<bool>,
        }
        validation::id(id)?;
        validation::revision(body.expected_revision)?;
        if body.key.is_empty() || body.key.len() > 4096 {
            return Err(Error::validation(
                "selected bridge key must contain 1 through 4096 bytes",
            ));
        }
        let value: BridgeCredentialStatus = self
            .client
            .operation(
                Method::PUT,
                &["bridges", id, "credential"],
                Some(&Import {
                    body,
                    retain_control_plane_authority,
                }),
                StatusCode::OK,
            )
            .await?;
        value.check()?;
        if !value.present || value.revoked {
            return Err(Error::protocol(
                "key import did not return active selected custody",
            ));
        }
        Ok(value)
    }
    /// Revoke exact selected custody under current management authority. This
    /// does not delete retained resources or alter the definition revision.
    pub async fn revoke_key(
        &self,
        id: &str,
        expected_revision: u64,
        expected_custody_revision: u64,
        key: &IdempotencyKey,
    ) -> Result<BridgeCredentialStatus> {
        validation::id(id)?;
        validation::revision(expected_revision)?;
        mutation_revision(expected_custody_revision)?;
        management(key)?;
        let value: BridgeCredentialStatus = self
            .client
            .command(
                Method::POST,
                &["bridges", id, "credential", "revoke"],
                Some(&Revoke {
                    expected_revision,
                    expected_custody_revision,
                }),
                key,
                StatusCode::OK,
            )
            .await?;
        value.check()?;
        if !value.present || !value.revoked || value.revision != expected_custody_revision + 1 {
            return Err(Error::protocol(
                "key revocation receipt does not match the requested custody revision",
            ));
        }
        Ok(value)
    }
}
