//! Legal-document acceptances kept on the owner's retained directory row.
//!
//! Acceptances are an additive `legal_consents` array on the `users` document
//! keyed by subject. Directory readers ignore fields they do not model, and
//! the sign-in observation merges into the stored row, so neither an older
//! reader nor a later sign-in refuses or drops them. Account erasure removes
//! the whole row. When retention is enabled, the erasure worker preserves
//! detached acceptances separately. Writes never replace the directory row,
//! and a row with an unknown schema version is never written.
//!
//! Consent never creates the row. A `users` row is registration evidence, so
//! only the sign-in callback, after its registration decision, may create it.
//! An owner without one is refused until a browser sign-in records it.

use async_trait::async_trait;
use layer2_core::legal_consent::{
    valid_version, ConsentAcceptance, LegalConsentStore, MAX_RETAINED_ACCEPTANCES,
};
use layer2_core::{StoreError, SubjectId};
use mongodb::{
    bson::{doc, Document},
    Collection, Database,
};
use serde::Deserialize;

use super::{
    directory::USERS,
    durability::majority_collection,
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

pub(super) const FIELD: &str = "legal_consents";

#[derive(Deserialize)]
struct ConsentRow {
    document_schema_version: i64,
    #[serde(default)]
    legal_consents: Vec<StoredAcceptance>,
}

/// One stored entry. Fields a later release adds are ignored, so an older
/// reader keeps reading rows written after an upgrade.
#[derive(Deserialize)]
struct StoredAcceptance {
    version: String,
    accepted_at: String,
}

pub(super) fn decode(row: Document) -> Result<Vec<ConsentAcceptance>, StoreError> {
    let row: ConsentRow = mongodb::bson::deserialize_from_document(row)
        .map_err(|_| StoreError::Malformed("legal consent record is not readable".into()))?;
    schema::require_current(row.document_schema_version)?;
    let mut versions = std::collections::BTreeSet::new();
    if row.legal_consents.len() > MAX_RETAINED_ACCEPTANCES
        || row.legal_consents.iter().any(|record| {
            !valid_version(&record.version)
                || layer2_core::seconds_from_rfc3339(&record.accepted_at).is_none()
                || !versions.insert(&record.version)
        })
    {
        return Err(StoreError::Malformed(
            "legal consent record is not readable".into(),
        ));
    }
    Ok(row
        .legal_consents
        .into_iter()
        .map(|record| ConsentAcceptance {
            version: record.version,
            accepted_at: record.accepted_at,
        })
        .collect())
}

pub struct MongoLegalConsentStore {
    users: Collection<Document>,
}

impl MongoLegalConsentStore {
    /// The directory store initializes and verifies the shared collection.
    pub(super) fn new(database: &Database) -> Self {
        Self {
            users: majority_collection(database, USERS),
        }
    }

    async fn recorded(&self, subject: &SubjectId) -> Result<Vec<ConsentAcceptance>, StoreError> {
        let Some(row) = self
            .users
            .find_one(doc! { "_id": subject.as_str() })
            .projection(doc! { "document_schema_version": 1, FIELD: 1 })
            .await
            .map_err(|_| StoreError::Unavailable("legal consent could not be read".into()))?
        else {
            return Ok(Vec::new());
        };
        decode(row)
    }
}

#[async_trait]
impl LegalConsentStore for MongoLegalConsentStore {
    async fn acceptance(
        &self,
        subject: &SubjectId,
        version: &str,
    ) -> Result<Option<ConsentAcceptance>, StoreError> {
        Ok(self
            .recorded(subject)
            .await?
            .into_iter()
            .find(|record| record.version == version))
    }

    async fn accept(
        &self,
        subject: &SubjectId,
        version: &str,
        at: &str,
    ) -> Result<ConsentAcceptance, StoreError> {
        let written = self
            .users
            .update_one(
                doc! {
                    "_id": subject.as_str(),
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    format!("{FIELD}.version"): { "$ne": version },
                },
                doc! { "$push": { FIELD: {
                    "$each": [{ "version": version, "accepted_at": at }],
                    "$slice": -(MAX_RETAINED_ACCEPTANCES as i64),
                } } },
            )
            .await
            .map_err(|_| StoreError::Unavailable("legal consent could not be recorded".into()))?;
        if written.matched_count == 1 {
            return Ok(ConsentAcceptance {
                version: version.to_owned(),
                accepted_at: at.to_owned(),
            });
        }
        // The version is already recorded, the row is not current, or there
        // is no row. Report the stored acceptance, never a second one.
        self.acceptance(subject, version).await?.ok_or_else(|| {
            StoreError::ReferenceUnavailable("the owner's sign-in record is not available".into())
        })
    }
}

#[cfg(test)]
#[path = "legal_consents_tests.rs"]
mod tests;
