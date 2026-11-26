//! Encrypted custody for one-time NyxID values awaiting sandbox delivery.

mod binding;
mod custody;
#[cfg(test)]
mod fixtures;
pub(super) mod issuance;
mod issuance_record;
#[cfg(test)]
mod provided_tests;
pub(super) mod purge;

use layer2_core::{seconds_from_rfc3339, SandboxId, StoreError, SubjectId};
use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::{Client, Collection, IndexModel};

use crate::auth::keys::valid_provider_key_id;
use crate::sandbox::{
    CredentialCustodyError, CredentialMaterial, CredentialRevocation, SandboxCredentialCustody,
};

use super::durability::majority_collection;
use super::encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope};
use super::schema::{initialize_collections, DOCUMENT_SCHEMA_VERSION};

pub(super) const SANDBOX_CREDENTIALS: &str = "sandbox_credentials";
pub(super) const SANDBOX_CREDENTIAL_REVOCATIONS: &str = "sandbox_credential_revocations";
const CREDENTIAL_KIND: &str = "sandbox-credential";

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialDocument {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    sandbox_id: String,
    owner: String,
    generation: i64,
    #[serde(
        default,
        skip_serializing_if = "layer2_core::sandbox::SandboxCredentialSource::is_managed"
    )]
    source: layer2_core::sandbox::SandboxCredentialSource,
    provider_key_id: String,
    provider_state_version: i64,
    issued_at: String,
    expires_at: String,
    grant_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    github_route: Option<layer3_adapter::GithubCredentialRoute>,
    value: CipherEnvelope,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RevocationDocument {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i64,
    sandbox_id: String,
    owner: String,
    provider_key_id: String,
}

pub(crate) struct MongoSandboxCredentialStore {
    client: Client,
    sandboxes: Collection<mongodb::bson::Document>,
    intents: Collection<mongodb::bson::Document>,
    collection: Collection<CredentialDocument>,
    revocations: Collection<RevocationDocument>,
    cipher: AuthCipher,
}

impl MongoSandboxCredentialStore {
    /// Read exact custody inside the caller's lineage transaction.
    pub(crate) async fn get_in_session(
        &self,
        sandbox: &SandboxId,
        generation: u64,
        session: &mut mongodb::ClientSession,
    ) -> Result<Option<CredentialMaterial>, StoreError> {
        self.collection
            .find_one(doc! {"_id": credential_id(sandbox, generation)})
            .session(session)
            .await
            .map_err(|_| StoreError::Unavailable("credential custody unavailable".into()))?
            .map(|document| self.material(document))
            .transpose()
    }

    pub(super) async fn open(
        client: &Client,
        database: &str,
        deployment: &str,
        key: &AuthEncryptionKey,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        initialize_collections(
            &database,
            &[
                SANDBOX_CREDENTIALS,
                SANDBOX_CREDENTIAL_REVOCATIONS,
                issuance::COLLECTION,
            ],
        )
        .await?;
        let collection = majority_collection(&database, SANDBOX_CREDENTIALS);
        let revocations = majority_collection(&database, SANDBOX_CREDENTIAL_REVOCATIONS);
        collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "sandbox_id": 1, "generation": 1 })
                    .options(
                        IndexOptions::builder()
                            .name("sandbox_credentials_generation".to_string())
                            .unique(true)
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("sandbox credential index: {error}"))
            })?;
        revocations
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "sandbox_id": 1 })
                    .options(
                        IndexOptions::builder()
                            .name("sandbox_credential_revocations_sandbox".to_string())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("sandbox credential revocation index: {error}"))
            })?;
        Ok(Self {
            client: client.clone(),
            sandboxes: majority_collection(&database, "sandboxes"),
            intents: majority_collection(&database, issuance::COLLECTION),
            collection,
            revocations,
            cipher: AuthCipher::new(deployment, key)?,
        })
    }

    fn document(&self, material: &CredentialMaterial) -> Result<CredentialDocument, StoreError> {
        validate_material(material)?;
        let authority = &material.authority;
        let id = credential_id(&material.sandbox_id, authority.generation);
        Ok(CredentialDocument {
            id: id.clone(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            sandbox_id: material.sandbox_id.0.clone(),
            owner: material.owner.0.clone(),
            generation: i64::try_from(authority.generation)
                .map_err(|_| StoreError::Malformed("credential generation is too large".into()))?,
            source: material.source.clone(),
            provider_key_id: authority.provider_key_id.clone(),
            provider_state_version: authority.provider_state_version,
            issued_at: authority.issued_at.clone(),
            expires_at: authority.expires_at.clone(),
            grant_fingerprint: authority.grant_fingerprint.clone(),
            github_route: material.github_route.clone(),
            value: self.cipher.seal(
                CREDENTIAL_KIND,
                id.as_bytes(),
                "value",
                material.value.expose(),
            )?,
        })
    }

    fn material(&self, document: CredentialDocument) -> Result<CredentialMaterial, StoreError> {
        if document.document_schema_version != DOCUMENT_SCHEMA_VERSION || document.generation <= 0 {
            return Err(StoreError::Malformed(
                "sandbox credential document has an unsupported shape".into(),
            ));
        }
        let expected = credential_id(
            &SandboxId(document.sandbox_id.clone()),
            u64::try_from(document.generation)
                .map_err(|_| StoreError::Malformed("credential generation is negative".into()))?,
        );
        if document.id != expected {
            return Err(StoreError::Malformed(
                "sandbox credential id disagrees with its authority".into(),
            ));
        }
        let material = CredentialMaterial {
            source: document.source,
            github_route: document.github_route,
            sandbox_id: SandboxId(document.sandbox_id),
            owner: SubjectId(document.owner),
            authority: layer2_core::sandbox::SandboxCredentialAuthority {
                generation: u64::try_from(document.generation)
                    .expect("positive i64 credential generation fits u64"),
                provider_key_id: document.provider_key_id,
                provider_state_version: document.provider_state_version,
                issued_at: document.issued_at,
                expires_at: document.expires_at,
                grant_fingerprint: document.grant_fingerprint,
            },
            value: crate::auth::persistence::SecretValue::new(self.cipher.open(
                CREDENTIAL_KIND,
                document.id.as_bytes(),
                "value",
                document.value,
            )?),
        };
        validate_material(&material)?;
        Ok(material)
    }

    fn revocation(&self, document: RevocationDocument) -> Result<CredentialRevocation, StoreError> {
        if document.document_schema_version != DOCUMENT_SCHEMA_VERSION {
            return Err(StoreError::Malformed(
                "sandbox credential revocation has an unsupported shape".into(),
            ));
        }
        let revocation = CredentialRevocation {
            sandbox_id: SandboxId(document.sandbox_id),
            owner: SubjectId(document.owner),
            provider_key_id: document.provider_key_id,
        };
        validate_revocation(&revocation)?;
        if document.id != revocation_id(&revocation.sandbox_id, &revocation.provider_key_id) {
            return Err(StoreError::Malformed(
                "sandbox credential revocation id disagrees with its authority".into(),
            ));
        }
        Ok(revocation)
    }
}

fn credential_id(sandbox_id: &SandboxId, generation: u64) -> String {
    format!("{}:{generation:020}", sandbox_id.as_str())
}

fn revocation_id(sandbox_id: &SandboxId, provider_key_id: &str) -> String {
    format!("{}:{provider_key_id}", sandbox_id.as_str())
}

fn validate_material(material: &CredentialMaterial) -> Result<(), StoreError> {
    let authority = &material.authority;
    let issued = seconds_from_rfc3339(&authority.issued_at);
    let expires = seconds_from_rfc3339(&authority.expires_at);
    if material.sandbox_id.as_str().trim().is_empty()
        || material.owner.as_str().trim().is_empty()
        || authority.generation == 0
        || !valid_provider_key_id(&authority.provider_key_id)
        || authority.provider_state_version <= 0
        || !material.matches_source()
        || issued.is_none()
        || (material.source.is_managed()
            && issued
                .zip(expires)
                .is_none_or(|(issued, expires)| expires <= issued))
        || authority.grant_fingerprint.len() != 64
        || !authority
            .grant_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(StoreError::Malformed(
            "sandbox credential material has an unsupported shape".into(),
        ));
    }
    Ok(())
}

fn validate_revocation(revocation: &CredentialRevocation) -> Result<(), StoreError> {
    if revocation.sandbox_id.as_str().trim().is_empty()
        || revocation.owner.as_str().trim().is_empty()
        || !valid_provider_key_id(&revocation.provider_key_id)
    {
        Err(StoreError::Malformed(
            "sandbox credential revocation has an unsupported shape".into(),
        ))
    } else {
        Ok(())
    }
}

fn custody_store_error(error: StoreError) -> CredentialCustodyError {
    match error {
        StoreError::Unavailable(detail) => CredentialCustodyError::Unavailable(detail),
        other => CredentialCustodyError::Corrupt(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_ids_are_generation_scoped() {
        assert_eq!(
            credential_id(&SandboxId("sbx_test".into()), 7),
            "sbx_test:00000000000000000007"
        );
    }

    #[test]
    fn custody_store_failures_keep_their_retryability() {
        assert!(matches!(
            custody_store_error(StoreError::Unavailable("offline".into())),
            CredentialCustodyError::Unavailable(_)
        ));
        assert!(matches!(
            custody_store_error(StoreError::Malformed("bad row".into())),
            CredentialCustodyError::Corrupt(_)
        ));
    }
}
