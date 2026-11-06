//! Encrypted MongoDB persistence for browser/OIDC authentication authority.

use std::time::Duration;

use layer2_core::{Authorization, StoreError, SubjectId};
use mongodb::{
    bson::{doc, spec::BinarySubtype, Binary, DateTime},
    options::IndexOptions,
    Client, Collection, IndexModel,
};
use serde::{Deserialize, Serialize};

use crate::auth::{
    persistence::{
        ApiBearerCustody, AuthStore, BrowserLogin, DurableSubjectTokens, LoginAttempt,
        SecretDigest, SecretValue, SubjectAuthState, SubjectRefreshLease,
    },
    profile::Profile,
};

use super::{
    durability::majority_collection,
    encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope},
    schema::{self, DOCUMENT_SCHEMA_VERSION},
};

pub(super) const BROWSER_LOGINS: &str = "auth_browser_logins";
pub(super) const LOGIN_ATTEMPTS: &str = "auth_login_attempts";
pub(super) const SUBJECTS: &str = "auth_subjects";
pub(super) const API_BEARERS: &str = "auth_api_bearers";
pub(super) const ACTOR_LINKS: &str = "auth_actor_links";
pub(super) const PROXY_GRANTS: &str = "auth_nyxid_proxy_grants";
pub(super) const AUTH_COLLECTIONS: &[&str] = &[
    BROWSER_LOGINS,
    LOGIN_ATTEMPTS,
    SUBJECTS,
    API_BEARERS,
    ACTOR_LINKS,
    PROXY_GRANTS,
];
const EXPIRY_INDEX: &str = "expires_at_ttl";
const LOGIN_KIND: &str = "login-attempt";
const SUBJECT_KIND: &str = "subject-auth";

#[path = "auth_actor_links.rs"]
mod actor_links;
#[path = "auth_api_bearers.rs"]
mod api_bearers;
#[path = "auth_login_documents.rs"]
mod login_documents;
#[path = "auth_proxy_grants.rs"]
mod proxy_grants;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrowserLoginDoc {
    #[serde(rename = "_id")]
    id: Binary,
    document_schema_version: i64,
    subject: String,
    expires_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoginAttemptDoc {
    #[serde(rename = "_id")]
    id: Binary,
    document_schema_version: i64,
    browser_binding: Binary,
    verifier: CipherEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    return_to: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    actor_link_intent: Option<CipherEnvelope>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    silent: bool,
    expires_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubjectDoc {
    #[serde(rename = "_id")]
    subject: String,
    document_schema_version: i64,
    incarnation: String,
    revision: u64,
    token_generation: u64,
    #[serde(deserialize_with = "required_refresh_lease")]
    refresh_lease: Option<RefreshLeaseDoc>,
    authorization: Authorization,
    profile: Profile,
    access_token: Option<CipherEnvelope>,
    refresh_token: Option<CipherEnvelope>,
    token_expires_at: Option<DateTime>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefreshLeaseDoc {
    holder: String,
    expires_at: DateTime,
}

fn required_refresh_lease<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RefreshLeaseDoc>, D::Error> {
    Option::deserialize(deserializer)
}

/// Durable authentication store. Resource credential custody is intentionally
/// a separate concern owned by the replacement resource service.
pub struct MongoAuthStore {
    browser_logins: Collection<BrowserLoginDoc>,
    login_attempts: Collection<LoginAttemptDoc>,
    subjects: Collection<SubjectDoc>,
    api_bearers: Collection<api_bearers::ApiBearerDoc>,
    actor_links: Collection<actor_links::ActorLinkDoc>,
    proxy_grants: Collection<proxy_grants::ProxyGrantDoc>,
    cipher: AuthCipher,
}

impl MongoAuthStore {
    pub(super) async fn open(
        client: &Client,
        database: &str,
        deployment: &str,
        encryption_key: &AuthEncryptionKey,
    ) -> Result<Self, StoreError> {
        let database = client.database(database);
        schema::initialize_collections(&database, AUTH_COLLECTIONS).await?;
        let store = Self {
            browser_logins: majority_collection(&database, BROWSER_LOGINS),
            login_attempts: majority_collection(&database, LOGIN_ATTEMPTS),
            subjects: majority_collection(&database, SUBJECTS),
            api_bearers: majority_collection(&database, API_BEARERS),
            actor_links: majority_collection(&database, ACTOR_LINKS),
            proxy_grants: majority_collection(&database, PROXY_GRANTS),
            cipher: AuthCipher::new(deployment, encryption_key)?,
        };
        let expiry = || {
            IndexModel::builder()
                .keys(doc! { "expires_at": 1 })
                .options(
                    IndexOptions::builder()
                        .name(EXPIRY_INDEX.to_owned())
                        .expire_after(Duration::ZERO)
                        .build(),
                )
                .build()
        };
        store
            .browser_logins
            .create_index(expiry())
            .await
            .map_err(|error| StoreError::Unavailable(format!("browser login index: {error}")))?;
        store
            .login_attempts
            .create_index(expiry())
            .await
            .map_err(|error| StoreError::Unavailable(format!("login attempt index: {error}")))?;
        store
            .api_bearers
            .create_index(expiry())
            .await
            .map_err(|_| StoreError::Unavailable("API bearer custody expiry index".into()))?;
        store
            .actor_links
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"link_id":1})
                    .options(IndexOptions::builder().unique(true).build())
                    .build(),
            )
            .await
            .map_err(|_| StoreError::Unavailable("actor link identity index".into()))?;
        store
            .actor_links
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"active":1,"wakeup_due_at":1,"_id":1})
                    .build(),
            )
            .await
            .map_err(|_| StoreError::Unavailable("actor wakeup index".into()))?;
        Ok(store)
    }

    fn subject_to_doc(&self, state: SubjectAuthState) -> Result<SubjectDoc, StoreError> {
        state.validate()?;
        let id = state.subject.as_str().as_bytes();
        let (access_token, refresh_token, token_expires_at) = match state.tokens {
            Some(tokens) => (
                Some(self.cipher.seal(
                    SUBJECT_KIND,
                    id,
                    "access_token",
                    tokens.access_token.expose(),
                )?),
                tokens
                    .refresh_token
                    .as_ref()
                    .map(|token| {
                        self.cipher
                            .seal(SUBJECT_KIND, id, "refresh_token", token.expose())
                    })
                    .transpose()?,
                Some(DateTime::from_system_time(tokens.expires_at)),
            ),
            None => (None, None, None),
        };
        Ok(SubjectDoc {
            subject: state.subject.0,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            incarnation: state.incarnation,
            revision: state.revision,
            token_generation: state.token_generation,
            refresh_lease: state.refresh_lease.map(|lease| RefreshLeaseDoc {
                holder: lease.holder,
                expires_at: DateTime::from_system_time(lease.expires_at),
            }),
            authorization: state.authorization,
            profile: state.profile,
            access_token,
            refresh_token,
            token_expires_at,
        })
    }

    fn subject_from_doc(&self, document: SubjectDoc) -> Result<SubjectAuthState, StoreError> {
        schema::require_current(document.document_schema_version)?;
        let id = document.subject.as_bytes();
        let tokens = match (
            document.access_token,
            document.refresh_token,
            document.token_expires_at,
        ) {
            (Some(access), refresh_token, Some(expires_at)) => Some(DurableSubjectTokens {
                access_token: SecretValue::new(self.cipher.open(
                    SUBJECT_KIND,
                    id,
                    "access_token",
                    access,
                )?),
                refresh_token: refresh_token
                    .map(|refresh| self.cipher.open(SUBJECT_KIND, id, "refresh_token", refresh))
                    .transpose()?
                    .map(SecretValue::new),
                expires_at: expires_at.to_system_time(),
            }),
            (None, None, None) => None,
            _ => {
                return Err(StoreError::Malformed(
                    "subject auth token fields are incomplete".into(),
                ))
            }
        };
        let state = SubjectAuthState {
            subject: SubjectId::new(document.subject),
            incarnation: document.incarnation,
            revision: document.revision,
            token_generation: document.token_generation,
            refresh_lease: document.refresh_lease.map(|lease| SubjectRefreshLease {
                holder: lease.holder,
                expires_at: lease.expires_at.to_system_time(),
            }),
            authorization: document.authorization,
            profile: document.profile,
            tokens,
        };
        state.validate()?;
        Ok(state)
    }
}

#[async_trait::async_trait]
impl AuthStore for MongoAuthStore {
    async fn due_actor_wakeups(
        &self,
        due_before: u64,
        limit: usize,
    ) -> Result<Vec<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
        self.actor_wakeups_due(due_before, limit).await
    }
    async fn due_actor_links(
        &self,
        renewed_before: u64,
        limit: usize,
    ) -> Result<Vec<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
        self.actor_links_due(renewed_before, limit).await
    }
    async fn actor_link_for_actor(
        &self,
        actor_key: &str,
    ) -> Result<Option<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
        self.read_actor_link(doc! {"_id":actor_key}).await
    }
    async fn actor_link_by_id(
        &self,
        link_id: &str,
    ) -> Result<Option<crate::auth::actor_links::ActorLinkRecord>, StoreError> {
        self.read_actor_link(doc! {"link_id":link_id}).await
    }
    async fn replace_actor_link(
        &self,
        expected_revision: Option<u64>,
        link: crate::auth::actor_links::ActorLinkRecord,
    ) -> Result<bool, StoreError> {
        self.write_actor_link(expected_revision, link).await
    }
    async fn insert_api_bearer(&self, custody: ApiBearerCustody) -> Result<bool, StoreError> {
        self.insert_api_bearer_record(custody).await
    }

    async fn insert_proxy_grant(
        &self,
        grant: crate::auth::nyxid_proxy::ProxyGrant,
    ) -> Result<bool, StoreError> {
        self.insert_proxy_grant_record(grant).await
    }

    async fn proxy_grant(
        &self,
        id: &[u8; 32],
    ) -> Result<Option<crate::auth::nyxid_proxy::ProxyGrant>, StoreError> {
        self.read_proxy_grant_record(id).await
    }

    async fn api_bearer(&self, id: &SecretDigest) -> Result<Option<ApiBearerCustody>, StoreError> {
        self.read_api_bearer_record(id).await
    }

    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError> {
        self.login_attempts
            .insert_one(self.login_to_doc(attempt)?)
            .await
            .map_err(|error| StoreError::Unavailable(format!("store login attempt: {error}")))?;
        Ok(())
    }

    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
        browser_binding: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        self.login_attempts
            .find_one_and_delete(doc! {
                "_id": binary(*id.as_bytes()),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "browser_binding": binary(*browser_binding.as_bytes()),
            })
            .await
            .map_err(|error| StoreError::Unavailable(format!("take login attempt: {error}")))?
            .map(|document| self.login_from_doc(document))
            .transpose()
    }

    async fn put_browser_login(&self, login: BrowserLogin) -> Result<(), StoreError> {
        let id = binary(*login.id.as_bytes());
        self.browser_logins
            .replace_one(
                doc! {
                    "_id": &id,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                },
                Self::browser_login_to_doc(login),
            )
            .upsert(true)
            .await
            .map_err(|error| StoreError::Unavailable(format!("store browser login: {error}")))?;
        Ok(())
    }

    async fn browser_login(&self, id: &SecretDigest) -> Result<Option<BrowserLogin>, StoreError> {
        self.browser_logins
            .find_one(doc! { "_id": binary(*id.as_bytes()) })
            .await
            .map_err(|error| StoreError::Unavailable(format!("read browser login: {error}")))?
            .map(|document| {
                schema::require_current(document.document_schema_version)?;
                Ok(BrowserLogin {
                    id: digest(&document.id, "browser login")?,
                    subject: SubjectId::new(document.subject),
                    expires_at: document.expires_at.to_system_time(),
                })
            })
            .transpose()
    }

    async fn delete_browser_login(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.browser_logins
            .delete_one(doc! {
                "_id": binary(*id.as_bytes()),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            })
            .await
            .map_err(|error| StoreError::Unavailable(format!("delete browser login: {error}")))?;
        Ok(())
    }

    async fn insert_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError> {
        match self.subjects.insert_one(self.subject_to_doc(state)?).await {
            Ok(_) => Ok(true),
            Err(error) if super::sandbox_mongo::is_duplicate_key(&error) => Ok(false),
            Err(error) => Err(StoreError::Unavailable(format!(
                "insert subject auth: {error}"
            ))),
        }
    }

    async fn replace_unfenced_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError> {
        let document = self.subject_to_doc(state)?;
        let result = self
            .subjects
            .replace_one(
                doc! {
                    "_id": &document.subject,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "incarnation": { "$exists": false },
                    "revision": { "$exists": false },
                    "token_generation": { "$exists": false },
                    "refresh_lease": { "$exists": false },
                },
                document,
            )
            .await
            .map_err(|error| {
                StoreError::Unavailable(format!("replace unfenced subject auth: {error}"))
            })?;
        Ok(result.matched_count == 1)
    }

    async fn replace_subject(
        &self,
        expected_incarnation: &str,
        expected_revision: u64,
        state: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        state.validate_replacement(expected_revision)?;
        let subject = state.subject.0.clone();
        let result = self.subjects
            .replace_one(
                doc! {
                    "_id": &subject,
                    "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                    "incarnation": expected_incarnation,
                    "revision": i64::try_from(expected_revision)
                        .map_err(|_| StoreError::Malformed("subject auth revision exceeds BSON".into()))?,
                },
                self.subject_to_doc(state)?,
            )
            .await
            .map_err(|error| StoreError::Unavailable(format!("replace subject auth: {error}")))?;
        Ok(result.matched_count == 1)
    }

    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.subjects
            .clone_with_type::<mongodb::bson::Document>()
            .find_one(doc! { "_id": subject.as_str() })
            .await
            .map_err(|error| StoreError::Unavailable(format!("read subject auth: {error}")))?
            .map(|document| {
                let document =
                    mongodb::bson::deserialize_from_document(document).map_err(|error| {
                        StoreError::Malformed(format!("subject auth document is invalid: {error}"))
                    })?;
                self.subject_from_doc(document)
            })
            .transpose()
    }
}

fn binary(bytes: [u8; 32]) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes: bytes.to_vec(),
    }
}

fn digest(value: &Binary, description: &str) -> Result<SecretDigest, StoreError> {
    if value.subtype != BinarySubtype::Generic {
        return Err(StoreError::Malformed(format!(
            "{description} has an unsupported BSON subtype"
        )));
    }
    let bytes = value.bytes.as_slice().try_into().map_err(|_| {
        StoreError::Malformed(format!("{description} must contain exactly 32 bytes"))
    })?;
    Ok(SecretDigest::from_bytes(bytes))
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "auth_refresh_tests.rs"]
mod refresh_tests;

#[cfg(test)]
#[path = "auth_api_bearer_tests.rs"]
mod api_bearer_tests;
