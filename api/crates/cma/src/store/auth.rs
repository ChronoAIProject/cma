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
        AuthStore, BrowserLogin, DurableSubjectTokens, LoginAttempt, SecretDigest, SecretValue,
        SubjectAuthState,
    },
    profile::Profile,
};

use super::{
    durability::majority_collection,
    encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope},
};

const BROWSER_LOGINS: &str = "auth_browser_logins";
const LOGIN_ATTEMPTS: &str = "auth_login_attempts";
const SUBJECTS: &str = "auth_subjects";
const EXPIRY_INDEX: &str = "expires_at_ttl";
const LOGIN_KIND: &str = "login-attempt";
const SUBJECT_KIND: &str = "subject-auth";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrowserLoginDoc {
    #[serde(rename = "_id")]
    id: Binary,
    subject: String,
    expires_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LoginAttemptDoc {
    #[serde(rename = "_id")]
    id: Binary,
    browser_binding: Binary,
    verifier: CipherEnvelope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    return_to: Option<String>,
    expires_at: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SubjectDoc {
    #[serde(rename = "_id")]
    subject: String,
    authorization: Authorization,
    profile: Profile,
    access_token: Option<CipherEnvelope>,
    refresh_token: Option<CipherEnvelope>,
    token_expires_at: Option<DateTime>,
}

/// Durable authentication store. Resource credential custody is intentionally
/// a separate concern owned by the replacement resource service.
pub struct MongoAuthStore {
    browser_logins: Collection<BrowserLoginDoc>,
    login_attempts: Collection<LoginAttemptDoc>,
    subjects: Collection<SubjectDoc>,
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
        let store = Self {
            browser_logins: majority_collection(&database, BROWSER_LOGINS),
            login_attempts: majority_collection(&database, LOGIN_ATTEMPTS),
            subjects: majority_collection(&database, SUBJECTS),
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
        Ok(store)
    }

    fn login_to_doc(&self, attempt: LoginAttempt) -> Result<LoginAttemptDoc, StoreError> {
        let id = *attempt.id.as_bytes();
        Ok(LoginAttemptDoc {
            id: binary(id),
            browser_binding: binary(*attempt.browser_binding.as_bytes()),
            verifier: self
                .cipher
                .seal(LOGIN_KIND, &id, "verifier", attempt.verifier.expose())?,
            return_to: attempt.return_to,
            expires_at: DateTime::from_system_time(attempt.expires_at),
        })
    }

    fn login_from_doc(&self, document: LoginAttemptDoc) -> Result<LoginAttempt, StoreError> {
        let id = digest(&document.id, "login attempt")?;
        let browser_binding = digest(&document.browser_binding, "login browser binding")?;
        let verifier =
            self.cipher
                .open(LOGIN_KIND, id.as_bytes(), "verifier", document.verifier)?;
        Ok(LoginAttempt {
            id,
            browser_binding,
            verifier: SecretValue::new(verifier),
            return_to: document.return_to,
            expires_at: document.expires_at.to_system_time(),
        })
    }

    fn subject_to_doc(&self, state: SubjectAuthState) -> Result<SubjectDoc, StoreError> {
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
            authorization: state.authorization,
            profile: state.profile,
            access_token,
            refresh_token,
            token_expires_at,
        })
    }

    fn subject_from_doc(&self, document: SubjectDoc) -> Result<SubjectAuthState, StoreError> {
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
        Ok(SubjectAuthState {
            subject: SubjectId::new(document.subject),
            authorization: document.authorization,
            profile: document.profile,
            tokens,
        })
    }
}

#[async_trait::async_trait]
impl AuthStore for MongoAuthStore {
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
                "browser_binding": binary(*browser_binding.as_bytes()),
            })
            .await
            .map_err(|error| StoreError::Unavailable(format!("take login attempt: {error}")))?
            .map(|document| self.login_from_doc(document))
            .transpose()
    }

    async fn put_browser_login(&self, login: BrowserLogin) -> Result<(), StoreError> {
        self.browser_logins
            .replace_one(
                doc! { "_id": binary(*login.id.as_bytes()) },
                BrowserLoginDoc {
                    id: binary(*login.id.as_bytes()),
                    subject: login.subject.0,
                    expires_at: DateTime::from_system_time(login.expires_at),
                },
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
            .delete_one(doc! { "_id": binary(*id.as_bytes()) })
            .await
            .map_err(|error| StoreError::Unavailable(format!("delete browser login: {error}")))?;
        Ok(())
    }

    async fn put_subject(&self, state: SubjectAuthState) -> Result<(), StoreError> {
        let subject = state.subject.0.clone();
        self.subjects
            .replace_one(doc! { "_id": &subject }, self.subject_to_doc(state)?)
            .upsert(true)
            .await
            .map_err(|error| StoreError::Unavailable(format!("store subject auth: {error}")))?;
        Ok(())
    }

    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.subjects
            .find_one(doc! { "_id": subject.as_str() })
            .await
            .map_err(|error| StoreError::Unavailable(format!("read subject auth: {error}")))?
            .map(|document| self.subject_from_doc(document))
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
