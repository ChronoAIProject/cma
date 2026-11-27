//! Shared values used by authentication and its persistence adapter.

use std::time::SystemTime;

use layer2_core::{Authorization, StoreError, SubjectId};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroize;

use super::profile::Profile;

/// A secret value that redacts diagnostics and clears owned bytes on drop.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    pub fn new(value: String) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "SecretValue(<redacted {} bytes>)", self.0.len())
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// One-way identifier for a secret presented by a browser.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretDigest([u8; 32]);

impl SecretDigest {
    pub fn identify(domain: &str, value: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(domain.as_bytes());
        digest.update([0]);
        digest.update(value.as_bytes());
        Self(digest.finalize().into())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SecretDigest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretDigest(<one-way>)")
    }
}

#[derive(Debug, Clone)]
pub struct BrowserLogin {
    pub id: SecretDigest,
    pub subject: SubjectId,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurableSubjectTokens {
    pub access_token: SecretValue,
    pub refresh_token: Option<SecretValue>,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectRefreshLease {
    pub holder: String,
    pub expires_at: SystemTime,
}

#[derive(Debug, Clone)]
pub struct SubjectAuthState {
    pub subject: SubjectId,
    pub incarnation: String,
    pub revision: u64,
    pub token_generation: u64,
    pub refresh_lease: Option<SubjectRefreshLease>,
    pub authorization: Authorization,
    pub profile: Profile,
    pub tokens: Option<DurableSubjectTokens>,
}

impl SubjectAuthState {
    pub fn new(
        subject: SubjectId,
        authorization: Authorization,
        profile: Profile,
        tokens: Option<DurableSubjectTokens>,
    ) -> Self {
        Self {
            subject,
            incarnation: super::token::random_token(),
            revision: 1,
            token_generation: 1,
            refresh_lease: None,
            authorization,
            profile,
            tokens,
        }
    }

    pub fn validate(&self) -> Result<(), StoreError> {
        if self.incarnation.trim().is_empty()
            || self.revision == 0
            || self.revision > i64::MAX as u64
            || self.token_generation == 0
            || self.token_generation > self.revision
            || self.refresh_lease.as_ref().is_some_and(|lease| {
                lease.holder.trim().is_empty()
                    || lease.expires_at <= SystemTime::UNIX_EPOCH
                    || self.tokens.is_none()
            })
        {
            return Err(StoreError::Malformed(
                "subject auth fencing is invalid".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_replacement(&self, expected_revision: u64) -> Result<(), StoreError> {
        self.validate()?;
        if self.revision != next_auth_counter(expected_revision)? {
            return Err(StoreError::Malformed(
                "subject auth revision must advance once".into(),
            ));
        }
        Ok(())
    }
}

pub fn next_auth_counter(value: u64) -> Result<u64, StoreError> {
    value
        .checked_add(1)
        .filter(|next| *next <= i64::MAX as u64)
        .ok_or_else(|| StoreError::Malformed("subject auth counter exhausted".into()))
}
