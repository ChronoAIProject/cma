//! Owner acceptance of the deployment's published terms and privacy notice.
//!
//! A deployment publishes at most one current version of its legal documents.
//! Each verified human owner records acceptance of an exact version once; the
//! original acceptance time is retained when the same version is accepted
//! again, and earlier acceptances remain readable after a new version is
//! published. The documents themselves are owned and hosted by the operator;
//! this module records only their version and the owner's acceptance.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{account_deletion::OwnerSessionAuthority, Caller, StoreError, SubjectId};

/// Longest accepted version label.
pub const MAX_VERSION_LENGTH: usize = 64;
/// Longest accepted document URL.
pub const MAX_URL_LENGTH: usize = 2048;
/// Acceptances retained per owner; older ones are dropped first. Readers
/// refuse longer histories, so this bound may grow but must never shrink.
pub const MAX_RETAINED_ACCEPTANCES: usize = 16;

/// The legal documents a deployment currently asks owners to accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalTerms {
    version: String,
    terms_url: String,
    privacy_url: String,
}

impl LegalTerms {
    /// Validate an operator-configured version and its two document URLs.
    pub fn new(version: &str, terms_url: &str, privacy_url: &str) -> Result<Self, String> {
        if !valid_version(version) {
            return Err(format!(
                "legal terms version must be 1-{MAX_VERSION_LENGTH} characters of A-Z, a-z, 0-9, '.', '_' or '-'"
            ));
        }
        for (name, url) in [("terms", terms_url), ("privacy", privacy_url)] {
            if !valid_document_url(url) {
                return Err(format!(
                    "legal {name} URL must be an absolute https URL of at most {MAX_URL_LENGTH} characters"
                ));
            }
        }
        Ok(Self {
            version: version.to_owned(),
            terms_url: terms_url.to_owned(),
            privacy_url: privacy_url.to_owned(),
        })
    }

    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn terms_url(&self) -> &str {
        &self.terms_url
    }

    pub fn privacy_url(&self) -> &str {
        &self.privacy_url
    }
}

/// Whether a version label is in the closed accepted alphabet.
pub fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= MAX_VERSION_LENGTH
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_document_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    url.len() <= MAX_URL_LENGTH
        && !host.is_empty()
        && !host.contains('@')
        && url.bytes().all(|byte| byte.is_ascii_graphic())
}

/// One recorded acceptance of an exact version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentAcceptance {
    pub version: String,
    pub accepted_at: String,
}

/// The current documents and whether this owner has accepted them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegalConsentStatus {
    pub terms: LegalTerms,
    pub accepted_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LegalConsentError {
    /// This deployment publishes no legal documents.
    #[error("legal terms are not configured")]
    NotConfigured,
    /// Only a verified human owner session may read or record consent.
    #[error("legal consent requires a verified owner session")]
    RequiresOwnerSession,
    /// The caller accepted a version other than the current one.
    #[error("legal terms version is not current")]
    VersionMismatch,
    /// The owner's sign-in record is missing; a new browser sign-in records it.
    #[error("legal consent requires a recorded sign-in")]
    SignInRequired,
    /// The consent record could not be read or written.
    #[error("legal consent store failed: {0}")]
    Store(String),
}

impl From<StoreError> for LegalConsentError {
    fn from(error: StoreError) -> Self {
        Self::Store(error.to_string())
    }
}

/// Durable per-owner acceptance records.
#[async_trait]
pub trait LegalConsentStore: Send + Sync {
    /// This owner's acceptance of exactly `version`, if recorded.
    async fn acceptance(
        &self,
        subject: &SubjectId,
        version: &str,
    ) -> Result<Option<ConsentAcceptance>, StoreError>;

    /// Record acceptance once. Accepting a recorded version again returns the
    /// original acceptance unchanged. `ReferenceUnavailable` means the owner
    /// has no sign-in record to attach the acceptance to.
    async fn accept(
        &self,
        subject: &SubjectId,
        version: &str,
        at: &str,
    ) -> Result<ConsentAcceptance, StoreError>;
}

pub struct LegalConsentService {
    terms: Option<LegalTerms>,
    store: Arc<dyn LegalConsentStore>,
}

impl LegalConsentService {
    /// `terms` is `None` when the deployment publishes no legal documents.
    pub fn new(terms: Option<LegalTerms>, store: Arc<dyn LegalConsentStore>) -> Self {
        Self { terms, store }
    }

    /// The currently published documents, safe to show before sign-in.
    pub fn terms(&self) -> Option<&LegalTerms> {
        self.terms.as_ref()
    }

    pub async fn status(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
    ) -> Result<LegalConsentStatus, LegalConsentError> {
        let terms = self.authorized_terms(caller, authority)?;
        let accepted = self
            .store
            .acceptance(&caller.subject, terms.version())
            .await?;
        Ok(LegalConsentStatus {
            terms: terms.clone(),
            accepted_at: accepted.map(|record| record.accepted_at),
        })
    }

    pub async fn accept(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
        version: &str,
    ) -> Result<LegalConsentStatus, LegalConsentError> {
        let terms = self.authorized_terms(caller, authority)?;
        if version != terms.version() {
            return Err(LegalConsentError::VersionMismatch);
        }
        let record = self
            .store
            .accept(&caller.subject, terms.version(), &crate::now_rfc3339())
            .await
            .map_err(|error| match error {
                StoreError::ReferenceUnavailable(_) => LegalConsentError::SignInRequired,
                error => error.into(),
            })?;
        if record.version != terms.version() {
            return Err(LegalConsentError::Store(
                "stored acceptance names another version".into(),
            ));
        }
        Ok(LegalConsentStatus {
            terms: terms.clone(),
            accepted_at: Some(record.accepted_at),
        })
    }

    fn authorized_terms(
        &self,
        caller: &Caller,
        authority: Option<&OwnerSessionAuthority>,
    ) -> Result<&LegalTerms, LegalConsentError> {
        if authority.is_none_or(|authority| authority.subject() != &caller.subject) {
            return Err(LegalConsentError::RequiresOwnerSession);
        }
        self.terms.as_ref().ok_or(LegalConsentError::NotConfigured)
    }
}

/// In-process store for tests and contract fixtures.
#[derive(Default)]
pub struct MemoryLegalConsentStore {
    accepted: Mutex<BTreeMap<String, Vec<ConsentAcceptance>>>,
}

impl MemoryLegalConsentStore {
    fn records(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, BTreeMap<String, Vec<ConsentAcceptance>>>, StoreError>
    {
        self.accepted
            .lock()
            .map_err(|_| StoreError::Unavailable("legal consent memory store poisoned".into()))
    }
}

#[async_trait]
impl LegalConsentStore for MemoryLegalConsentStore {
    async fn acceptance(
        &self,
        subject: &SubjectId,
        version: &str,
    ) -> Result<Option<ConsentAcceptance>, StoreError> {
        Ok(self
            .records()?
            .get(subject.as_str())
            .and_then(|records| records.iter().find(|record| record.version == version))
            .cloned())
    }

    async fn accept(
        &self,
        subject: &SubjectId,
        version: &str,
        at: &str,
    ) -> Result<ConsentAcceptance, StoreError> {
        let mut accepted = self.records()?;
        let records = accepted.entry(subject.as_str().to_owned()).or_default();
        if let Some(record) = records.iter().find(|record| record.version == version) {
            return Ok(record.clone());
        }
        let record = ConsentAcceptance {
            version: version.to_owned(),
            accepted_at: at.to_owned(),
        };
        records.push(record.clone());
        if records.len() > MAX_RETAINED_ACCEPTANCES {
            records.remove(0);
        }
        Ok(record)
    }
}

#[cfg(test)]
#[path = "legal_consent_tests.rs"]
mod tests;
