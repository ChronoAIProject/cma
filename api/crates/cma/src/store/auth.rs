//! Durable browser, OAuth, subject, and session-credential state in MongoDB.

use std::time::Duration;

use futures_util::TryStreamExt;
use layer2_core::store::StoreError;
use layer2_core::{Authorization, Capability, SessionId, SubjectId};
use mongodb::bson::{doc, spec::BinarySubtype, Binary, DateTime};
use mongodb::error::{Error, ErrorKind, WriteFailure};
use mongodb::options::{IndexOptions, ReplaceOptions};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

use crate::auth::persistence::{
    AuthStore, BrowserSession, CredentialLease, DurableCredential, DurableSubjectTokens,
    LoginAttempt, RevokeDebt, SecretDigest, SecretValue, SessionCredentialState, SubjectAuthState,
};
use crate::auth::profile::Profile;

use super::durability::majority_collection;
use super::encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope};

const BROWSER_SESSIONS: &str = "auth_browser_sessions";
const LOGIN_ATTEMPTS: &str = "auth_login_attempts";
const SUBJECTS: &str = "auth_subjects";
const CREDENTIALS: &str = "auth_session_credentials";
const BROWSER_EXPIRY_INDEX: &str = "auth_browser_session_expiry";
const LOGIN_EXPIRY_INDEX: &str = "auth_login_attempt_expiry";
const DUPLICATE_KEY: i32 = 11000;

const LOGIN_KIND: &str = "login-attempt";
const SUBJECT_KIND: &str = "subject-auth";
const CREDENTIAL_KIND: &str = "session-credential";
const VERIFIER_FIELD: &str = "verifier";
const ACCESS_TOKEN_FIELD: &str = "tokens.access_token";
const REFRESH_TOKEN_FIELD: &str = "tokens.refresh_token";
const CURRENT_VALUE_FIELD: &str = "current.value";
const PENDING_VALUE_FIELD: &str = "pending.value";

#[derive(Serialize, Deserialize)]
struct BrowserSessionDoc {
    #[serde(rename = "_id")]
    id: Binary,
    subject: String,
    expires_at: DateTime,
}

#[derive(Serialize, Deserialize)]
struct LoginAttemptDoc {
    #[serde(rename = "_id")]
    id: Binary,
    verifier: CipherEnvelope,
    expires_at: DateTime,
}

#[derive(Serialize, Deserialize)]
struct SubjectTokensDoc {
    access_token: CipherEnvelope,
    refresh_token: Option<CipherEnvelope>,
    expires_at: DateTime,
}

#[derive(Serialize, Deserialize)]
struct ProfileDoc {
    display_name: Option<String>,
    email: Option<String>,
    avatar_url: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct SubjectAuthDoc {
    #[serde(rename = "_id")]
    id: String,
    revision: i64,
    tokens: Option<SubjectTokensDoc>,
    capabilities: Vec<String>,
    profile: ProfileDoc,
}

#[derive(Serialize, Deserialize)]
struct CredentialDoc {
    id: String,
    value: CipherEnvelope,
    owner: String,
    issued_at: DateTime,
    expires_at: Option<DateTime>,
    scope_fingerprint: String,
}

#[derive(Serialize, Deserialize)]
struct RevokeDebtDoc {
    id: String,
    owner: String,
    queued_at: DateTime,
}

#[derive(Serialize, Deserialize)]
struct CredentialLeaseDoc {
    holder: String,
    fence: i64,
    acquired_at: DateTime,
    expires_at: DateTime,
    scope_fingerprint: String,
}

#[derive(Serialize, Deserialize)]
struct SessionCredentialDoc {
    #[serde(rename = "_id")]
    id: String,
    generation: i64,
    current: Option<CredentialDoc>,
    pending: Option<CredentialDoc>,
    revoke_debt: Vec<RevokeDebtDoc>,
    lease: Option<CredentialLeaseDoc>,
    retired: bool,
}

/// MongoDB-backed authority for every authentication fact.
pub struct MongoAuthStore {
    browser_sessions: Collection<BrowserSessionDoc>,
    login_attempts: Collection<LoginAttemptDoc>,
    subjects: Collection<SubjectAuthDoc>,
    credentials: Collection<SessionCredentialDoc>,
    cipher: AuthCipher,
}

impl MongoAuthStore {
    pub(super) async fn open(
        client: &Client,
        database: &str,
        deployment: &str,
        encryption_key: &AuthEncryptionKey,
    ) -> Result<Self, StoreError> {
        let db = client.database(database);
        let store = Self {
            browser_sessions: majority_collection(&db, BROWSER_SESSIONS),
            login_attempts: majority_collection(&db, LOGIN_ATTEMPTS),
            subjects: majority_collection(&db, SUBJECTS),
            credentials: majority_collection(&db, CREDENTIALS),
            cipher: AuthCipher::new(deployment, encryption_key)?,
        };
        store.ensure_indexes().await?;
        tracing::info!(database, "authentication store ready");
        Ok(store)
    }

    async fn ensure_indexes(&self) -> Result<(), StoreError> {
        self.browser_sessions
            .create_index(expiry_index(BROWSER_EXPIRY_INDEX))
            .await
            .map_err(|_| {
                StoreError::Unavailable("browser session index: database operation failed".into())
            })?;
        self.login_attempts
            .create_index(expiry_index(LOGIN_EXPIRY_INDEX))
            .await
            .map_err(|_| {
                StoreError::Unavailable("login attempt index: database operation failed".into())
            })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl AuthStore for MongoAuthStore {
    async fn put_browser_session(&self, session: BrowserSession) -> Result<(), StoreError> {
        let stored = browser_to_doc(session);
        self.browser_sessions
            .replace_one(doc! { "_id": stored.id.clone() }, &stored)
            .with_options(ReplaceOptions::builder().upsert(true).build())
            .await
            .map_err(|_| unavailable("store browser session"))?;
        Ok(())
    }

    async fn browser_session(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<BrowserSession>, StoreError> {
        self.browser_sessions
            .find_one(doc! { "_id": id_binary(id) })
            .await
            .map_err(|error| read_failure("read browser session", &error))?
            .map(browser_from_doc)
            .transpose()
    }

    async fn delete_browser_session(&self, id: &SecretDigest) -> Result<(), StoreError> {
        self.browser_sessions
            .delete_one(doc! { "_id": id_binary(id) })
            .await
            .map_err(|_| unavailable("delete browser session"))?;
        Ok(())
    }

    async fn put_login_attempt(&self, attempt: LoginAttempt) -> Result<(), StoreError> {
        let stored = login_to_doc(attempt, &self.cipher)?;
        self.login_attempts
            .replace_one(doc! { "_id": stored.id.clone() }, &stored)
            .with_options(ReplaceOptions::builder().upsert(true).build())
            .await
            .map_err(|_| unavailable("store login attempt"))?;
        Ok(())
    }

    async fn take_login_attempt(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<LoginAttempt>, StoreError> {
        self.login_attempts
            .find_one_and_delete(doc! { "_id": id_binary(id) })
            .await
            .map_err(|error| read_failure("consume login attempt", &error))?
            .map(|stored| login_from_doc(stored, &self.cipher))
            .transpose()
    }

    async fn subject(&self, subject: &SubjectId) -> Result<Option<SubjectAuthState>, StoreError> {
        self.subjects
            .find_one(doc! { "_id": subject.as_str() })
            .await
            .map_err(|error| read_failure("read subject authentication", &error))?
            .map(|stored| subject_from_doc(stored, &self.cipher))
            .transpose()
    }

    async fn insert_subject(&self, state: SubjectAuthState) -> Result<bool, StoreError> {
        if state.revision != 0 {
            return Err(StoreError::Malformed(
                "new subject auth state must start at revision zero".into(),
            ));
        }
        let stored = subject_to_doc(state, &self.cipher)?;
        match self.subjects.insert_one(stored).await {
            Ok(_) => Ok(true),
            Err(error) if is_duplicate_key(&error) => Ok(false),
            Err(_) => Err(unavailable("insert subject authentication")),
        }
    }

    async fn replace_subject(
        &self,
        expected_revision: u64,
        state: SubjectAuthState,
    ) -> Result<bool, StoreError> {
        validate_next(expected_revision, state.revision, "subject auth")?;
        let expected = stored_counter(expected_revision, "subject auth revision")?;
        let stored = subject_to_doc(state, &self.cipher)?;
        let result = self
            .subjects
            .replace_one(doc! { "_id": &stored.id, "revision": expected }, &stored)
            .await
            .map_err(|_| unavailable("replace subject authentication"))?;
        Ok(result.matched_count == 1)
    }

    async fn subjects(&self) -> Result<Vec<SubjectAuthState>, StoreError> {
        let mut cursor = self
            .subjects
            .find(doc! {})
            .await
            .map_err(|error| read_failure("list subject authentication", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor
            .try_next()
            .await
            .map_err(|error| read_failure("list subject authentication cursor", &error))?
        {
            states.push(subject_from_doc(stored, &self.cipher)?);
        }
        Ok(states)
    }

    async fn credential(
        &self,
        session: &SessionId,
    ) -> Result<Option<SessionCredentialState>, StoreError> {
        self.credentials
            .find_one(doc! { "_id": session.as_str() })
            .await
            .map_err(|error| read_failure("read session credential", &error))?
            .map(|stored| credential_state_from_doc(stored, &self.cipher))
            .transpose()
    }

    async fn insert_credential(&self, state: SessionCredentialState) -> Result<bool, StoreError> {
        if state.generation != 0 {
            return Err(StoreError::Malformed(
                "new credential state must start at generation zero".into(),
            ));
        }
        let stored = credential_state_to_doc(state, &self.cipher)?;
        match self.credentials.insert_one(stored).await {
            Ok(_) => Ok(true),
            Err(error) if is_duplicate_key(&error) => Ok(false),
            Err(_) => Err(unavailable("insert session credential")),
        }
    }

    async fn replace_credential(
        &self,
        expected_generation: u64,
        state: SessionCredentialState,
    ) -> Result<bool, StoreError> {
        validate_next(expected_generation, state.generation, "credential")?;
        let expected = stored_counter(expected_generation, "credential generation")?;
        let stored = credential_state_to_doc(state, &self.cipher)?;
        let result = self
            .credentials
            .replace_one(doc! { "_id": &stored.id, "generation": expected }, &stored)
            .await
            .map_err(|_| unavailable("replace session credential"))?;
        Ok(result.matched_count == 1)
    }

    async fn credentials(&self) -> Result<Vec<SessionCredentialState>, StoreError> {
        let mut cursor = self
            .credentials
            .find(doc! {})
            .await
            .map_err(|error| read_failure("list session credentials", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor
            .try_next()
            .await
            .map_err(|error| read_failure("list session credentials cursor", &error))?
        {
            states.push(credential_state_from_doc(stored, &self.cipher)?);
        }
        Ok(states)
    }
}

fn expiry_index(name: &str) -> IndexModel {
    IndexModel::builder()
        .keys(doc! { "expires_at": 1 })
        .options(
            IndexOptions::builder()
                .name(name.to_string())
                .expire_after(Duration::ZERO)
                .build(),
        )
        .build()
}

fn unavailable(operation: &str) -> StoreError {
    StoreError::Unavailable(format!("{operation}: database operation failed"))
}

fn read_failure(operation: &str, error: &Error) -> StoreError {
    match error.kind.as_ref() {
        ErrorKind::Bson(_) | ErrorKind::BsonDeserialization(_) => {
            StoreError::Malformed(format!("{operation}: stored document is malformed"))
        }
        _ => unavailable(operation),
    }
}

fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

fn validate_next(current: u64, next: u64, record: &str) -> Result<(), StoreError> {
    if current.checked_add(1) != Some(next) {
        return Err(StoreError::Malformed(format!(
            "{record} revision must advance exactly once: {current} -> {next}"
        )));
    }
    Ok(())
}

fn stored_counter(value: u64, name: &str) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::Malformed(format!("{name} cannot be represented in BSON")))
}

fn read_counter(value: i64, name: &str) -> Result<u64, StoreError> {
    u64::try_from(value)
        .map_err(|_| StoreError::Malformed(format!("stored {name} must not be negative")))
}

fn id_binary(id: &SecretDigest) -> Binary {
    Binary {
        subtype: BinarySubtype::Generic,
        bytes: id.as_bytes().to_vec(),
    }
}

fn digest_from_binary(id: Binary, record: &str) -> Result<SecretDigest, StoreError> {
    if id.subtype != BinarySubtype::Generic {
        return Err(StoreError::Malformed(format!(
            "{record} id has an unsupported BSON subtype"
        )));
    }
    let bytes: [u8; 32] = id.bytes.try_into().map_err(|bytes: Vec<u8>| {
        StoreError::Malformed(format!(
            "{record} id has {} bytes instead of 32",
            bytes.len()
        ))
    })?;
    Ok(SecretDigest::from_bytes(bytes))
}

fn browser_to_doc(session: BrowserSession) -> BrowserSessionDoc {
    BrowserSessionDoc {
        id: id_binary(&session.id),
        subject: session.subject.0,
        expires_at: DateTime::from_system_time(session.expires_at),
    }
}

fn browser_from_doc(stored: BrowserSessionDoc) -> Result<BrowserSession, StoreError> {
    Ok(BrowserSession {
        id: digest_from_binary(stored.id, "browser session")?,
        subject: SubjectId(stored.subject),
        expires_at: stored.expires_at.to_system_time(),
    })
}

fn login_to_doc(attempt: LoginAttempt, cipher: &AuthCipher) -> Result<LoginAttemptDoc, StoreError> {
    Ok(LoginAttemptDoc {
        id: id_binary(&attempt.id),
        verifier: cipher.seal(
            LOGIN_KIND,
            attempt.id.as_bytes(),
            VERIFIER_FIELD,
            attempt.verifier.expose(),
        )?,
        expires_at: DateTime::from_system_time(attempt.expires_at),
    })
}

fn login_from_doc(
    stored: LoginAttemptDoc,
    cipher: &AuthCipher,
) -> Result<LoginAttempt, StoreError> {
    let id = digest_from_binary(stored.id, "login attempt")?;
    let verifier = cipher.open(LOGIN_KIND, id.as_bytes(), VERIFIER_FIELD, stored.verifier)?;
    Ok(LoginAttempt {
        id,
        verifier: SecretValue::new(verifier),
        expires_at: stored.expires_at.to_system_time(),
    })
}

fn subject_to_doc(
    state: SubjectAuthState,
    cipher: &AuthCipher,
) -> Result<SubjectAuthDoc, StoreError> {
    let SubjectAuthState {
        subject,
        revision,
        tokens,
        authorization,
        profile,
    } = state;
    let id = subject.0;
    let tokens = tokens
        .map(|tokens| {
            Ok(SubjectTokensDoc {
                access_token: cipher.seal(
                    SUBJECT_KIND,
                    id.as_bytes(),
                    ACCESS_TOKEN_FIELD,
                    tokens.access_token.expose(),
                )?,
                refresh_token: tokens
                    .refresh_token
                    .as_ref()
                    .map(|value| {
                        cipher.seal(
                            SUBJECT_KIND,
                            id.as_bytes(),
                            REFRESH_TOKEN_FIELD,
                            value.expose(),
                        )
                    })
                    .transpose()?,
                expires_at: DateTime::from_system_time(tokens.expires_at),
            })
        })
        .transpose()?;
    Ok(SubjectAuthDoc {
        id,
        revision: stored_counter(revision, "subject auth revision")?,
        tokens,
        capabilities: authorization
            .capabilities()
            .map(capability_name)
            .map(str::to_string)
            .collect(),
        profile: ProfileDoc {
            display_name: profile.display_name,
            email: profile.email,
            avatar_url: profile.avatar_url,
        },
    })
}

fn subject_from_doc(
    stored: SubjectAuthDoc,
    cipher: &AuthCipher,
) -> Result<SubjectAuthState, StoreError> {
    let id = stored.id;
    let tokens = stored
        .tokens
        .map(|tokens| {
            Ok(DurableSubjectTokens {
                access_token: SecretValue::new(cipher.open(
                    SUBJECT_KIND,
                    id.as_bytes(),
                    ACCESS_TOKEN_FIELD,
                    tokens.access_token,
                )?),
                refresh_token: tokens
                    .refresh_token
                    .map(|value| {
                        cipher
                            .open(SUBJECT_KIND, id.as_bytes(), REFRESH_TOKEN_FIELD, value)
                            .map(SecretValue::new)
                    })
                    .transpose()?,
                expires_at: tokens.expires_at.to_system_time(),
            })
        })
        .transpose()?;
    let capabilities = stored
        .capabilities
        .iter()
        .map(|name| capability(name))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SubjectAuthState {
        subject: SubjectId(id),
        revision: read_counter(stored.revision, "subject auth revision")?,
        tokens,
        authorization: Authorization::from_capabilities(capabilities),
        profile: Profile {
            display_name: stored.profile.display_name,
            email: stored.profile.email,
            avatar_url: stored.profile.avatar_url,
        },
    })
}

fn credential_state_to_doc(
    state: SessionCredentialState,
    cipher: &AuthCipher,
) -> Result<SessionCredentialDoc, StoreError> {
    let SessionCredentialState {
        session,
        generation,
        current,
        pending,
        revoke_debt,
        lease,
        retired,
    } = state;
    let id = session.0;
    Ok(SessionCredentialDoc {
        current: current
            .map(|value| credential_to_doc(value, cipher, &id, CURRENT_VALUE_FIELD))
            .transpose()?,
        pending: pending
            .map(|value| credential_to_doc(value, cipher, &id, PENDING_VALUE_FIELD))
            .transpose()?,
        revoke_debt: revoke_debt
            .into_iter()
            .map(|debt| RevokeDebtDoc {
                id: debt.id,
                owner: debt.owner.0,
                queued_at: DateTime::from_system_time(debt.queued_at),
            })
            .collect(),
        lease: lease
            .map(|lease| {
                Ok(CredentialLeaseDoc {
                    holder: lease.holder,
                    fence: stored_counter(lease.fence, "credential lease fence")?,
                    acquired_at: DateTime::from_system_time(lease.acquired_at),
                    expires_at: DateTime::from_system_time(lease.expires_at),
                    scope_fingerprint: lease.scope_fingerprint,
                })
            })
            .transpose()?,
        id,
        generation: stored_counter(generation, "credential generation")?,
        retired,
    })
}

fn credential_to_doc(
    credential: DurableCredential,
    cipher: &AuthCipher,
    record_id: &str,
    field: &str,
) -> Result<CredentialDoc, StoreError> {
    Ok(CredentialDoc {
        id: credential.id,
        value: cipher.seal(
            CREDENTIAL_KIND,
            record_id.as_bytes(),
            field,
            credential.value.expose(),
        )?,
        owner: credential.owner.0,
        issued_at: DateTime::from_system_time(credential.issued_at),
        expires_at: credential.expires_at.map(DateTime::from_system_time),
        scope_fingerprint: credential.scope_fingerprint,
    })
}

fn credential_state_from_doc(
    stored: SessionCredentialDoc,
    cipher: &AuthCipher,
) -> Result<SessionCredentialState, StoreError> {
    let id = stored.id;
    Ok(SessionCredentialState {
        current: stored
            .current
            .map(|value| credential_from_doc(value, cipher, &id, CURRENT_VALUE_FIELD))
            .transpose()?,
        pending: stored
            .pending
            .map(|value| credential_from_doc(value, cipher, &id, PENDING_VALUE_FIELD))
            .transpose()?,
        revoke_debt: stored
            .revoke_debt
            .into_iter()
            .map(|debt| RevokeDebt {
                id: debt.id,
                owner: SubjectId(debt.owner),
                queued_at: debt.queued_at.to_system_time(),
            })
            .collect(),
        lease: stored
            .lease
            .map(|lease| {
                Ok(CredentialLease {
                    holder: lease.holder,
                    fence: read_counter(lease.fence, "credential lease fence")?,
                    acquired_at: lease.acquired_at.to_system_time(),
                    expires_at: lease.expires_at.to_system_time(),
                    scope_fingerprint: lease.scope_fingerprint,
                })
            })
            .transpose()?,
        session: SessionId(id),
        generation: read_counter(stored.generation, "credential generation")?,
        retired: stored.retired,
    })
}

fn credential_from_doc(
    stored: CredentialDoc,
    cipher: &AuthCipher,
    record_id: &str,
    field: &str,
) -> Result<DurableCredential, StoreError> {
    Ok(DurableCredential {
        id: stored.id,
        value: SecretValue::new(cipher.open(
            CREDENTIAL_KIND,
            record_id.as_bytes(),
            field,
            stored.value,
        )?),
        owner: SubjectId(stored.owner),
        issued_at: stored.issued_at.to_system_time(),
        expires_at: stored.expires_at.map(|value| value.to_system_time()),
        scope_fingerprint: stored.scope_fingerprint,
    })
}

fn capability_name(capability: Capability) -> &'static str {
    match capability {
        Capability::CreateSession => "create_session",
        Capability::UseOwnSessions => "use_own_sessions",
        Capability::TerminateOwnSession => "terminate_own_session",
        Capability::ManageOwnEnvironmentProfiles => "manage_own_environment_profiles",
        Capability::ReadAllSessions => "read_all_sessions",
        Capability::ReadRuntimeInventory => "read_runtime_inventory",
        Capability::ReadEventLog => "read_event_log",
        Capability::ReadActivityLog => "read_activity_log",
        Capability::ReadUserDirectory => "read_user_directory",
        Capability::ConfigurePlatform => "configure_platform",
        Capability::ControlAnySession => "control_any_session",
        Capability::ExceedSessionLimit => "exceed_session_limit",
    }
}

fn capability(name: &str) -> Result<Capability, StoreError> {
    match name {
        "create_session" => Ok(Capability::CreateSession),
        "use_own_sessions" => Ok(Capability::UseOwnSessions),
        "terminate_own_session" => Ok(Capability::TerminateOwnSession),
        "manage_own_environment_profiles" => Ok(Capability::ManageOwnEnvironmentProfiles),
        "read_all_sessions" => Ok(Capability::ReadAllSessions),
        "read_runtime_inventory" => Ok(Capability::ReadRuntimeInventory),
        "read_event_log" => Ok(Capability::ReadEventLog),
        "read_activity_log" => Ok(Capability::ReadActivityLog),
        "read_user_directory" => Ok(Capability::ReadUserDirectory),
        "configure_platform" => Ok(Capability::ConfigurePlatform),
        "control_any_session" => Ok(Capability::ControlAnySession),
        "exceed_session_limit" => Ok(Capability::ExceedSessionLimit),
        other => Err(StoreError::Malformed(format!(
            "subject auth names unknown capability {other:?}"
        ))),
    }
}

#[cfg(test)]
#[path = "auth_tests.rs"]
mod tests;
