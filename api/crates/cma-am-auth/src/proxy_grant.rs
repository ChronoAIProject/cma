//! Retained authorization from a verified identity proxy assertion.

use layer2_core::{Authorization, StoreError, SubjectId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Immutable admitted proxy authority, containing no access/refresh token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProxyGrant {
    pub id: [u8; 32],
    pub issuer: String,
    pub service_id: String,
    pub assertion_id: String,
    pub subject: SubjectId,
    pub authorization: Authorization,
    pub owner_incarnation: String,
}

impl ProxyGrant {
    pub fn identity(issuer: &str, service: &str, subject: &SubjectId, assertion: &str) -> [u8; 32] {
        Sha256::digest(
            serde_json::to_vec(&(
                "nyxid-proxy-admission-v1",
                issuer,
                service,
                subject,
                assertion,
            ))
            .expect("string tuple serializes"),
        )
        .into()
    }
    pub fn validate(&self) -> Result<(), StoreError> {
        if self.issuer.is_empty()
            || self.service_id.is_empty()
            || self.assertion_id.is_empty()
            || self.subject.as_str().is_empty()
            || self.owner_incarnation.is_empty()
            || self.issuer.len() > 2048
            || self.service_id.len() > 256
            || self.subject.as_str().len() > 256
            || self.assertion_id.len() > 256
            || self.owner_incarnation.len() > 256
            || self.id
                != Self::identity(
                    &self.issuer,
                    &self.service_id,
                    &self.subject,
                    &self.assertion_id,
                )
        {
            return Err(StoreError::Malformed(
                "invalid NyxID proxy admission".into(),
            ));
        }
        Ok(())
    }
}
