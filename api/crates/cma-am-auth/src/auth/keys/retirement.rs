use super::{valid_provider_key_id, validate_credential_id, KeyIssuer, KeyIssuerError};
use layer2_core::SubjectId;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

const MAX_RECEIPT_BYTES: usize = 8 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RetirementDisposition {
    Revoked,
    NoIssuance,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRetirementReceipt {
    operation_id: String,
    owner: String,
    request_sha256: String,
    receipt_id: String,
    revision: i64,
    state: RetirementDisposition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
}

impl CredentialRetirementReceipt {
    pub fn matches(&self, operation: &str, owner: &SubjectId, digest: &str) -> bool {
        self.operation_id == operation
            && self.owner == owner.as_str()
            && self.request_sha256 == digest
            && valid_provider_key_id(&self.receipt_id)
            && self.revision > 0
            && match self.state {
                RetirementDisposition::Revoked => {
                    self.key_id.as_deref().is_some_and(valid_provider_key_id)
                }
                RetirementDisposition::NoIssuance => self.key_id.is_none(),
            }
    }
}

impl KeyIssuer {
    /// Fetch the issuer's immutable retirement disposition for a known mint.
    pub async fn retirement_receipt(
        &self,
        operation: &str,
        owner: &SubjectId,
        request_digest: &str,
        owner_token: &str,
    ) -> Result<Option<CredentialRetirementReceipt>, KeyIssuerError> {
        validate_credential_id(operation)?;
        let mut response = self
            .http
            .get(format!(
                "{}/api/v1/api-key-issuances/{operation}/retirement",
                self.base_url
            ))
            .query(&[("request_sha256", request_digest)])
            .bearer_auth(owner_token)
            .send()
            .await
            .map_err(|_| receipt_unavailable())?;
        match response.status() {
            StatusCode::ACCEPTED | StatusCode::NOT_FOUND => return Ok(None),
            StatusCode::OK => {}
            status => return Err(KeyIssuerError::Refused(status.as_u16())),
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| receipt_unavailable())? {
            if chunk.len() > MAX_RECEIPT_BYTES - body.len() {
                return Err(KeyIssuerError::UnexpectedAuthority);
            }
            body.extend_from_slice(&chunk);
        }
        let receipt: CredentialRetirementReceipt =
            serde_json::from_slice(&body).map_err(|_| KeyIssuerError::UnexpectedAuthority)?;
        if !receipt.matches(operation, owner, request_digest) {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        Ok(Some(receipt))
    }
}

fn receipt_unavailable() -> KeyIssuerError {
    KeyIssuerError::Unavailable("credential retirement receipt unavailable".into())
}

#[cfg(test)]
mod tests;
