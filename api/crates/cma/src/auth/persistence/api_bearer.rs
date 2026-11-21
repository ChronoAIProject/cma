//! Exact, nonrenewable API credential custody, independent of browser sign-in.

use super::{SecretDigest, SecretValue};
use layer2_core::{StoreError, SubjectId};
use sha2::{Digest as _, Sha256};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiBearerCustody {
    pub id: SecretDigest,
    pub issuer: String,
    pub subject: SubjectId,
    pub credential_id: String,
    pub access_token: SecretValue,
    /// Exact provider-verified token expiry. Repeated admission cannot extend it.
    pub expires_at: Option<SystemTime>,
}

impl ApiBearerCustody {
    pub fn new(
        issuer: String,
        subject: SubjectId,
        credential_id: String,
        access_token: SecretValue,
        expires_at: impl Into<Option<SystemTime>>,
    ) -> Result<Self, StoreError> {
        let record = Self {
            id: Self::identity(&issuer, &subject, &credential_id)?,
            issuer,
            subject,
            credential_id,
            access_token,
            expires_at: expires_at.into(),
        };
        record.validate()?;
        Ok(record)
    }

    pub fn identity(
        issuer: &str,
        subject: &SubjectId,
        credential_id: &str,
    ) -> Result<SecretDigest, StoreError> {
        let bytes =
            serde_json::to_vec(&("cma-api-bearer-custody-v1", issuer, subject, credential_id))
                .map_err(|_| invalid())?;
        Ok(SecretDigest::from_bytes(Sha256::digest(bytes).into()))
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        let expiry = self
            .expires_at
            .map(|value| value.duration_since(SystemTime::UNIX_EPOCH))
            .transpose()
            .map_err(|_| invalid())?;
        if self.issuer.trim().is_empty()
            || self.issuer.len() > 2048
            || self.subject.as_str().trim().is_empty()
            || self.subject.as_str().len() > 256
            || self.credential_id.len() != 64
            || !self
                .credential_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || self.access_token.expose().is_empty()
            || self.access_token.expose().len() > 64 * 1024
            || self
                .access_token
                .expose()
                .bytes()
                .any(|byte| byte.is_ascii_whitespace() || byte.is_ascii_control())
            || expiry.is_some_and(|expiry| {
                expiry.is_zero()
                    || expiry.subsec_nanos() != 0
                    || expiry.as_secs() > i64::MAX as u64 / 1000
            })
            || self.id != Self::identity(&self.issuer, &self.subject, &self.credential_id)?
        {
            return Err(invalid());
        }
        Ok(())
    }
}

fn invalid() -> StoreError {
    StoreError::Malformed("API bearer custody identity is invalid".into())
}
