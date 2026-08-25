//! Durable browser, OAuth, subject, and session-credential state in MongoDB.

use std::time::{Duration, SystemTime};

use futures_util::TryStreamExt;
use layer2_core::store::StoreError;
use layer2_core::{Authorization, Capability, SessionId, SubjectId};
use mongodb::bson::{doc, spec::BinarySubtype, Binary, Bson, DateTime, Document};
use mongodb::error::{Error, ErrorKind, WriteFailure};
use mongodb::options::{Hint, IndexOptions, ReplaceOptions, ReturnDocument};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

use crate::auth::persistence::{
    AuthStore, AuthSweepItem, BrowserSession, CredentialLease, CredentialLeaseCursor,
    CredentialMutationCursor, CredentialReconciliationCursor, CredentialReissueCursor,
    DurableCredential, DurableSubjectTokens, LoginAttempt, RevokeDebt, RevokeDebtCursor,
    RevokeDebtWork, SecretDigest, SecretValue, SessionCredentialState, SubjectAuthState,
    SubjectOwnerCursor, SubjectRefreshCursor, SubjectRefreshLease,
};
use crate::auth::profile::Profile;

use super::durability::majority_collection;
use super::encryption::{AuthCipher, AuthEncryptionKey, CipherEnvelope};

const BROWSER_SESSIONS: &str = "auth_browser_sessions";
const LOGIN_ATTEMPTS: &str = "auth_login_attempts";
const SUBJECTS: &str = "auth_subjects";
const CREDENTIALS: &str = "auth_session_credentials";
const SWEEP_COUNTERS: &str = "auth_sweep_counters";
const SWEEP_COUNTER_ID: &str = "authentication";
const BROWSER_EXPIRY_INDEX: &str = "auth_browser_session_expiry";
const LOGIN_EXPIRY_INDEX: &str = "auth_login_attempt_expiry";
const SUBJECT_EXPIRY_INDEX: &str = "auth_subject_expiry";
const CREDENTIAL_EXPIRY_INDEX: &str = "auth_session_credential_expiry";
const SUBJECT_REFRESH_INDEX: &str = "auth_subject_refresh_due";
const SUBJECT_EXPIRED_REFRESH_LEASE_INDEX: &str = "auth_subject_expired_refresh_lease";
const SUBJECT_OWNER_INDEX: &str = "auth_subject_token_owners";
const CREDENTIAL_CURRENT_INDEX: &str = "auth_credential_current_due";
const CREDENTIAL_PENDING_INDEX: &str = "auth_credential_pending_due";
const CREDENTIAL_EXPIRED_LEASE_INDEX: &str = "auth_credential_expired_mint_lease";
const CREDENTIAL_DEBT_INDEX: &str = "auth_credential_revoke_debt";
const CREDENTIAL_DEBT_ORDER_INDEX: &str = "auth_credential_revoke_debt_order";
const CREDENTIAL_RECONCILIATION_INDEX: &str = "auth_active_credential_reconciliation";
const SUBJECT_MIGRATION_INDEX: &str = "auth_subject_schema_migration";
const CREDENTIAL_MIGRATION_INDEX: &str = "auth_credential_schema_migration";
const DUPLICATE_KEY: i32 = 11000;
const AUTH_SCHEMA_VERSION: i64 = 1;
const AUTH_MIGRATION_LANE_BATCH_SIZE: usize = 50;
const AUTH_SCHEMA_OPEN_BUDGET: usize = 2 * (AUTH_MIGRATION_LANE_BATCH_SIZE + 2) + 2;

/// A continuously refreshed delegation keeps rolling this deadline forward.
/// Thirty days of provider failure is finite while leaving ample room for a
/// transient outage to recover after the access token itself expires.
const SUBJECT_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Keep the terminal fence past every operation that could finish late.
///
/// One hour is deliberately longer than the 45-second refresh lease, the
/// 45-second mint lease, and the 15-second provider HTTP deadline. A late mint
/// therefore still encounters `retired` and records any revoke debt before the
/// otherwise empty tombstone becomes eligible for TTL cleanup.
const SETTLED_CREDENTIAL_RETENTION: Duration = Duration::from_secs(60 * 60);

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
    #[serde(default)]
    incarnation: String,
    revision: i64,
    #[serde(default)]
    token_generation: i64,
    #[serde(default)]
    sweep_sequence: i64,
    tokens: Option<SubjectTokensDoc>,
    #[serde(default)]
    refresh_lease: Option<SubjectRefreshLeaseDoc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delete_after: Option<DateTime>,
    #[serde(default)]
    auth_schema_version: i64,
    capabilities: Vec<String>,
    profile: ProfileDoc,
}

#[derive(Serialize, Deserialize)]
struct SubjectRefreshLeaseDoc {
    holder: String,
    expires_at: DateTime,
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

#[derive(Deserialize)]
struct RevokeDebtPageDoc {
    #[serde(rename = "_id")]
    id: String,
    revoke_debt: Vec<RevokeDebtDoc>,
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
    #[serde(default)]
    sweep_sequence: i64,
    current: Option<CredentialDoc>,
    pending: Option<CredentialDoc>,
    revoke_debt: Vec<RevokeDebtDoc>,
    #[serde(default)]
    revoke_debt_ordered: bool,
    lease: Option<CredentialLeaseDoc>,
    retired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delete_after: Option<DateTime>,
    #[serde(default)]
    auth_schema_version: i64,
}

#[derive(Deserialize)]
struct MigrationIdDoc {
    #[serde(rename = "_id")]
    id: String,
}

#[derive(Serialize, Deserialize)]
struct SweepCounterDoc {
    #[serde(rename = "_id")]
    id: String,
    value: i64,
}

/// MongoDB-backed authority for every authentication fact.
pub struct MongoAuthStore {
    browser_sessions: Collection<BrowserSessionDoc>,
    login_attempts: Collection<LoginAttemptDoc>,
    subjects: Collection<SubjectAuthDoc>,
    credentials: Collection<SessionCredentialDoc>,
    sweep_counters: Collection<SweepCounterDoc>,
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
            sweep_counters: majority_collection(&db, SWEEP_COUNTERS),
            cipher: AuthCipher::new(deployment, encryption_key)?,
        };
        store.ensure_indexes().await?;
        store.maintain_auth_schema(AUTH_SCHEMA_OPEN_BUDGET).await?;
        tracing::info!(database, "authentication store ready");
        Ok(store)
    }

    async fn allocate_sweep_sequences(&self, amount: usize) -> Result<Vec<i64>, StoreError> {
        if amount == 0 {
            return Ok(Vec::new());
        }
        let increment = i64::try_from(amount).map_err(|_| {
            StoreError::Malformed("authentication sweep allocation is too large".into())
        })?;
        let counter = self
            .sweep_counters
            .find_one_and_update(
                doc! { "_id": SWEEP_COUNTER_ID },
                doc! {
                    "$setOnInsert": { "_id": SWEEP_COUNTER_ID },
                    "$inc": { "value": increment },
                },
            )
            .upsert(true)
            .return_document(ReturnDocument::After)
            .await
            .map_err(|_| unavailable("allocate authentication sweep sequence"))?
            .ok_or_else(|| unavailable("read allocated authentication sweep sequence"))?;
        let first = counter
            .value
            .checked_sub(increment - 1)
            .filter(|first| *first > 0)
            .ok_or_else(|| {
                StoreError::Malformed("authentication sweep sequence exhausted".into())
            })?;
        Ok((first..=counter.value).collect())
    }

    async fn next_sweep_sequence(&self) -> Result<i64, StoreError> {
        self.allocate_sweep_sequences(1)
            .await?
            .pop()
            .ok_or_else(|| StoreError::Malformed("sweep sequence allocation was empty".into()))
    }

    async fn ensure_indexes(&self) -> Result<(), StoreError> {
        self.browser_sessions
            .create_index(expiry_index("expires_at", BROWSER_EXPIRY_INDEX))
            .await
            .map_err(|_| {
                StoreError::Unavailable("browser session index: database operation failed".into())
            })?;
        self.login_attempts
            .create_index(expiry_index("expires_at", LOGIN_EXPIRY_INDEX))
            .await
            .map_err(|_| {
                StoreError::Unavailable("login attempt index: database operation failed".into())
            })?;
        self.subjects
            .create_index(expiry_index("delete_after", SUBJECT_EXPIRY_INDEX))
            .await
            .map_err(|_| {
                StoreError::Unavailable("subject expiry index: database operation failed".into())
            })?;
        self.credentials
            .create_index(expiry_index("delete_after", CREDENTIAL_EXPIRY_INDEX))
            .await
            .map_err(|_| {
                StoreError::Unavailable("credential expiry index: database operation failed".into())
            })?;
        self.subjects
            .create_index(query_index(
                doc! { "tokens.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
                SUBJECT_REFRESH_INDEX,
                doc! {
                    "tokens.refresh_token": { "$type": "object" },
                    "refresh_lease": Bson::Null,
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("subject refresh query index"))?;
        self.subjects
            .create_index(query_index(
                doc! { "refresh_lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
                SUBJECT_EXPIRED_REFRESH_LEASE_INDEX,
                doc! {
                    "tokens.refresh_token": { "$type": "object" },
                    "refresh_lease.expires_at": { "$type": "date" },
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("expired subject refresh lease query index"))?;
        self.subjects
            .create_index(query_index(
                doc! { "sweep_sequence": 1, "_id": 1 },
                SUBJECT_OWNER_INDEX,
                doc! {
                    "tokens": { "$type": "object" },
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("subject token-owner query index"))?;
        self.credentials
            .create_index(query_index(
                doc! { "current.issued_at": 1, "sweep_sequence": 1, "_id": 1 },
                CREDENTIAL_CURRENT_INDEX,
                doc! {
                    "retired": false,
                    "lease": Bson::Null,
                    "pending": Bson::Null,
                    "current.issued_at": { "$type": "date" },
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("current credential query index"))?;
        self.credentials
            .create_index(query_index(
                doc! { "sweep_sequence": 1, "_id": 1 },
                CREDENTIAL_PENDING_INDEX,
                doc! {
                    "retired": false,
                    "lease": Bson::Null,
                    "pending.issued_at": { "$type": "date" },
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("pending credential query index"))?;
        self.credentials
            .create_index(query_index(
                doc! { "lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 },
                CREDENTIAL_EXPIRED_LEASE_INDEX,
                doc! {
                    "retired": false,
                    "pending": Bson::Null,
                    "current.issued_at": { "$type": "date" },
                    "lease.expires_at": { "$type": "date" },
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("expired credential mint lease query index"))?;
        self.credentials
            .create_index(query_index(
                doc! { "sweep_sequence": 1, "_id": 1, "revoke_debt.id": 1 },
                CREDENTIAL_DEBT_INDEX,
                doc! {
                    "revoke_debt.0": { "$exists": true },
                    "revoke_debt_ordered": true,
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("credential revoke-debt query index"))?;
        self.credentials
            .create_index(query_index(
                doc! { "revoke_debt_ordered": 1, "_id": 1 },
                CREDENTIAL_DEBT_ORDER_INDEX,
                doc! { "revoke_debt.0": { "$exists": true } },
            ))
            .await
            .map_err(|_| unavailable("credential revoke-debt order query index"))?;
        self.credentials
            .create_index(query_index(
                doc! { "sweep_sequence": 1, "_id": 1 },
                CREDENTIAL_RECONCILIATION_INDEX,
                doc! {
                    "retired": false,
                    "sweep_sequence": { "$type": "long" },
                },
            ))
            .await
            .map_err(|_| unavailable("active credential reconciliation query index"))?;
        self.subjects
            .create_index(named_index(
                doc! { "auth_schema_version": 1, "_id": 1 },
                SUBJECT_MIGRATION_INDEX,
            ))
            .await
            .map_err(|_| unavailable("subject schema migration index"))?;
        self.credentials
            .create_index(named_index(
                doc! { "auth_schema_version": 1, "_id": 1 },
                CREDENTIAL_MIGRATION_INDEX,
            ))
            .await
            .map_err(|_| unavailable("credential schema migration index"))?;
        Ok(())
    }

    async fn migrate_legacy_subjects(&self, max_operations: usize) -> Result<usize, StoreError> {
        // One list, one batched sequence allocation, then one update per row.
        if max_operations < 3 {
            return Ok(0);
        }
        let retention_millis = i64::try_from(SUBJECT_RETENTION.as_millis()).map_err(|_| {
            StoreError::Malformed("subject retention does not fit MongoDB milliseconds".into())
        })?;
        let limit = AUTH_MIGRATION_LANE_BATCH_SIZE.min(max_operations - 2);
        let ids = migration_ids(
            &self.subjects.clone_with_type::<MigrationIdDoc>(),
            SUBJECT_MIGRATION_INDEX,
            limit,
        )
        .await?;
        if ids.is_empty() {
            return Ok(1);
        }
        let operations = ids.len() + 2;
        let sequences = self.allocate_sweep_sequences(ids.len()).await?;
        let mut failed = false;
        for (id, sequence) in ids.into_iter().zip(sequences) {
            let incarnation = uuid::Uuid::new_v4().simple().to_string();
            if self
                .subjects
                .update_one(
                    doc! {
                        "_id": id,
                        "auth_schema_version": { "$ne": AUTH_SCHEMA_VERSION },
                    },
                    subject_migration_update(retention_millis, &incarnation, sequence),
                )
                .await
                .is_err()
            {
                failed = true;
            }
        }
        if failed {
            return Err(unavailable("migrate legacy subject authentication state"));
        }
        Ok(operations)
    }

    async fn migrate_legacy_credentials(&self, max_operations: usize) -> Result<usize, StoreError> {
        // One list, one batched sequence allocation, then one update per row.
        if max_operations < 3 {
            return Ok(0);
        }
        let retention_millis =
            i64::try_from(SETTLED_CREDENTIAL_RETENTION.as_millis()).map_err(|_| {
                StoreError::Malformed(
                    "credential retention does not fit MongoDB milliseconds".into(),
                )
            })?;
        let limit = AUTH_MIGRATION_LANE_BATCH_SIZE.min(max_operations - 2);
        let ids = migration_ids(
            &self.credentials.clone_with_type::<MigrationIdDoc>(),
            CREDENTIAL_MIGRATION_INDEX,
            limit,
        )
        .await?;
        if ids.is_empty() {
            return Ok(1);
        }
        let operations = ids.len() + 2;
        let sequences = self.allocate_sweep_sequences(ids.len()).await?;
        let mut failed = false;
        for (id, sequence) in ids.into_iter().zip(sequences) {
            if self
                .credentials
                .update_one(
                    doc! {
                        "_id": id,
                        "auth_schema_version": { "$ne": AUTH_SCHEMA_VERSION },
                    },
                    credential_migration_update(retention_millis, sequence),
                )
                .await
                .is_err()
            {
                failed = true;
            }
        }
        if failed {
            return Err(unavailable(
                "migrate legacy credential authentication state",
            ));
        }
        Ok(operations)
    }

    async fn normalize_one_revoke_debt_order(&self) -> Result<(), StoreError> {
        // An old replica in a rolling deployment can replace a record without
        // preserving the marker. Admit one such record per sweep call so the
        // compatibility work and its array sort remain bounded by one document.
        let sequence = self.next_sweep_sequence().await?;
        self.credentials
            .update_one(
                unordered_revoke_debt_filter(),
                revoke_debt_order_update(sequence),
            )
            .hint(Hint::Name(CREDENTIAL_DEBT_ORDER_INDEX.to_string()))
            .await
            .map_err(|_| unavailable("normalize credential revoke-debt order"))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl AuthStore for MongoAuthStore {
    async fn maintain_auth_schema(&self, max_operations: usize) -> Result<usize, StoreError> {
        // Keep both migration collections moving on every sufficiently sized
        // call, while reserving the final two operations for debt ordering.
        let debt_allowance = usize::from(max_operations >= 2) * 2;
        let migration_allowance = max_operations - debt_allowance;
        let subject_allowance = migration_allowance / 2;
        let credential_allowance = migration_allowance - subject_allowance;

        let mut used = 0;
        let mut first_error = None;
        match self.migrate_legacy_subjects(subject_allowance).await {
            Ok(operations) => used += operations,
            Err(error) => first_error = Some(error),
        }
        match self.migrate_legacy_credentials(credential_allowance).await {
            Ok(operations) => used += operations,
            Err(error) if first_error.is_none() => first_error = Some(error),
            Err(_) => {}
        }
        if debt_allowance == 2 {
            match self.normalize_one_revoke_debt_order().await {
                Ok(()) => used += 2,
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }

        first_error.map_or(Ok(used), Err)
    }

    async fn auth_sweep_high_water(&self) -> Result<u64, StoreError> {
        let value = self
            .sweep_counters
            .find_one(doc! { "_id": SWEEP_COUNTER_ID })
            .await
            .map_err(|error| read_failure("read authentication sweep high water", &error))?
            .map_or(0, |counter| counter.value);
        read_counter(value, "authentication sweep high water")
    }

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
        let mut stored = subject_to_doc(state, &self.cipher)?;
        stored.sweep_sequence = self.next_sweep_sequence().await?;
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
        let mut stored = subject_to_doc(state, &self.cipher)?;
        stored.sweep_sequence = self.next_sweep_sequence().await?;
        let result = self
            .subjects
            .replace_one(doc! { "_id": &stored.id, "revision": expected }, &stored)
            .await
            .map_err(|_| unavailable("replace subject authentication"))?;
        Ok(result.matched_count == 1)
    }

    async fn unleased_subjects_due_for_refresh(
        &self,
        refresh_at: SystemTime,
        after: Option<&SubjectRefreshCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let subjects = self.subjects.clone_with_type::<Document>();
        let mut cursor = subjects
            .find(unleased_subject_refresh_filter(
                refresh_at,
                after,
                through_sequence,
            ))
            .sort(doc! { "tokens.expires_at": 1, "sweep_sequence": 1, "_id": 1 })
            .hint(Hint::Name(SUBJECT_REFRESH_INDEX.to_string()))
            .limit(mongo_limit(limit))
            .await
            .map_err(|error| read_failure("list subjects due for refresh", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor
            .try_next()
            .await
            .map_err(|error| read_failure("list subjects due for refresh cursor", &error))?
        {
            states.push(AuthSweepItem {
                cursor: subject_refresh_cursor_from_document(&stored, "tokens", "expires_at")?,
                state: decode_subject_document(stored, &self.cipher),
            });
        }
        Ok(states)
    }

    async fn subjects_with_expired_refresh_leases(
        &self,
        now: SystemTime,
        after: Option<&SubjectRefreshCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<SubjectRefreshCursor, SubjectAuthState>>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let subjects = self.subjects.clone_with_type::<Document>();
        let mut cursor = subjects
            .find(expired_subject_refresh_lease_filter(
                now,
                after,
                through_sequence,
            ))
            .sort(doc! { "refresh_lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 })
            .hint(Hint::Name(SUBJECT_EXPIRED_REFRESH_LEASE_INDEX.to_string()))
            .limit(mongo_limit(limit))
            .await
            .map_err(|error| read_failure("list expired subject refresh leases", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor
            .try_next()
            .await
            .map_err(|error| read_failure("list expired subject refresh leases cursor", &error))?
        {
            states.push(AuthSweepItem {
                cursor: subject_refresh_cursor_from_document(
                    &stored,
                    "refresh_lease",
                    "expires_at",
                )?,
                state: decode_subject_document(stored, &self.cipher),
            });
        }
        Ok(states)
    }

    async fn subjects_with_tokens(
        &self,
        after: Option<&SubjectOwnerCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<SubjectOwnerCursor>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let subjects = self.subjects.clone_with_type::<Document>();
        let mut cursor = subjects
            .find(subject_owner_filter(after, through_sequence))
            .projection(doc! { "_id": 1, "sweep_sequence": 1 })
            .sort(doc! { "sweep_sequence": 1, "_id": 1 })
            .hint(subject_owner_hint())
            .limit(mongo_limit(limit))
            .await
            .map_err(|error| read_failure("list subjects with tokens", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor
            .try_next()
            .await
            .map_err(|error| read_failure("list subjects with tokens cursor", &error))?
        {
            states.push(subject_owner_cursor_from_document(&stored)?);
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
        let mut stored = credential_state_to_doc(state, &self.cipher)?;
        stored.sweep_sequence = self.next_sweep_sequence().await?;
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
        let mut stored = credential_state_to_doc(state, &self.cipher)?;
        stored.sweep_sequence = self.next_sweep_sequence().await?;
        let result = self
            .credentials
            .replace_one(doc! { "_id": &stored.id, "generation": expected }, &stored)
            .await
            .map_err(|_| unavailable("replace session credential"))?;
        Ok(result.matched_count == 1)
    }

    async fn current_credentials_due_for_reissue(
        &self,
        issued_before: SystemTime,
        after: Option<&CredentialReissueCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialReissueCursor, SessionCredentialState>>, StoreError>
    {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let credentials = self.credentials.clone_with_type::<Document>();
        let mut cursor = credentials
            .find(current_credential_reissue_filter(
                issued_before,
                after,
                through_sequence,
            ))
            .sort(doc! { "current.issued_at": 1, "sweep_sequence": 1, "_id": 1 })
            .hint(Hint::Name(CREDENTIAL_CURRENT_INDEX.to_string()))
            .limit(mongo_limit(limit))
            .await
            .map_err(|error| read_failure("list current credentials due for reissue", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor.try_next().await.map_err(|error| {
            read_failure("list current credentials due for reissue cursor", &error)
        })? {
            states.push(AuthSweepItem {
                cursor: credential_reissue_cursor_from_document(&stored)?,
                state: decode_credential_document(stored, &self.cipher),
            });
        }
        Ok(states)
    }

    async fn pending_credentials_due_for_reissue(
        &self,
        after: Option<&CredentialMutationCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialMutationCursor, SessionCredentialState>>, StoreError>
    {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let credentials = self.credentials.clone_with_type::<Document>();
        let mut cursor = credentials
            .find(pending_credential_reissue_filter(after, through_sequence))
            .sort(doc! { "sweep_sequence": 1, "_id": 1 })
            .hint(Hint::Name(CREDENTIAL_PENDING_INDEX.to_string()))
            .limit(mongo_limit(limit))
            .await
            .map_err(|error| read_failure("list pending credentials due for reissue", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor.try_next().await.map_err(|error| {
            read_failure("list pending credentials due for reissue cursor", &error)
        })? {
            states.push(AuthSweepItem {
                cursor: credential_mutation_cursor_from_document(&stored)?,
                state: decode_credential_document(stored, &self.cipher),
            });
        }
        Ok(states)
    }

    async fn credentials_with_expired_mint_leases(
        &self,
        now: SystemTime,
        after: Option<&CredentialLeaseCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<CredentialLeaseCursor, SessionCredentialState>>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let credentials = self.credentials.clone_with_type::<Document>();
        let mut cursor = credentials
            .find(expired_credential_mint_lease_filter(
                now,
                after,
                through_sequence,
            ))
            .sort(doc! { "lease.expires_at": 1, "sweep_sequence": 1, "_id": 1 })
            .hint(Hint::Name(CREDENTIAL_EXPIRED_LEASE_INDEX.to_string()))
            .limit(mongo_limit(limit))
            .await
            .map_err(|error| read_failure("list expired credential mint leases", &error))?;
        let mut states = Vec::new();
        while let Some(stored) = cursor
            .try_next()
            .await
            .map_err(|error| read_failure("list expired credential mint leases cursor", &error))?
        {
            states.push(AuthSweepItem {
                cursor: credential_lease_cursor_from_document(&stored)?,
                state: decode_credential_document(stored, &self.cipher),
            });
        }
        Ok(states)
    }

    async fn revoke_debts(
        &self,
        after: Option<&RevokeDebtCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<AuthSweepItem<RevokeDebtCursor, RevokeDebtWork>>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let debts = self.credentials.clone_with_type::<Document>();
        let mut work = Vec::with_capacity(limit);
        let mut position = after.cloned();
        while work.len() < limit {
            if let Some(after) = position.as_ref() {
                let stored = debts
                    .find_one(revoke_debt_after_filter(after, through_sequence))
                    .projection(revoke_debt_after_projection(&after.credential_id))
                    .hint(Hint::Name(CREDENTIAL_DEBT_INDEX.to_string()))
                    .await
                    .map_err(|error| read_failure("resume credential revoke debt", &error))?;
                if let Some(stored) = stored {
                    let next = revoke_debt_page_item(stored)?;
                    position = Some(next.cursor.clone());
                    work.push(next);
                    continue;
                }
            }

            let mut cursor = debts
                .find(credential_debt_parent_filter(
                    position.as_ref(),
                    through_sequence,
                ))
                .projection(first_revoke_debt_projection())
                .sort(doc! { "sweep_sequence": 1, "_id": 1 })
                .hint(Hint::Name(CREDENTIAL_DEBT_INDEX.to_string()))
                .limit(1)
                .await
                .map_err(|error| read_failure("list next credential revoke debt parent", &error))?;
            let Some(stored) = cursor.try_next().await.map_err(|error| {
                read_failure("list next credential revoke debt parent cursor", &error)
            })?
            else {
                break;
            };
            let next = revoke_debt_page_item(stored)?;
            position = Some(next.cursor.clone());
            work.push(next);
        }
        Ok(work)
    }

    async fn active_credentials_for_reconciliation(
        &self,
        after: Option<&CredentialReconciliationCursor>,
        through_sequence: u64,
        limit: usize,
    ) -> Result<Vec<CredentialReconciliationCursor>, StoreError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let credentials = self.credentials.clone_with_type::<Document>();
        let mut cursor = credentials
            .find(active_credential_reconciliation_filter(
                after,
                through_sequence,
            ))
            .projection(doc! { "_id": 1, "sweep_sequence": 1 })
            .sort(doc! { "sweep_sequence": 1, "_id": 1 })
            .hint(Hint::Name(CREDENTIAL_RECONCILIATION_INDEX.to_string()))
            .limit(mongo_limit(limit))
            .await
            .map_err(|error| read_failure("list credential reconciliation state", &error))?;
        let mut positions = Vec::new();
        while let Some(stored) = cursor
            .try_next()
            .await
            .map_err(|error| read_failure("list credential reconciliation state cursor", &error))?
        {
            positions.push(credential_reconciliation_cursor_from_document(&stored)?);
        }
        Ok(positions)
    }
}

fn expiry_index(field: &str, name: &str) -> IndexModel {
    let mut keys = Document::new();
    keys.insert(field, 1);
    IndexModel::builder()
        .keys(keys)
        .options(
            IndexOptions::builder()
                .name(name.to_string())
                .expire_after(Duration::ZERO)
                .build(),
        )
        .build()
}

fn query_index(keys: Document, name: &str, partial_filter: Document) -> IndexModel {
    IndexModel::builder()
        .keys(keys)
        .options(
            IndexOptions::builder()
                .name(name.to_string())
                .partial_filter_expression(partial_filter)
                .build(),
        )
        .build()
}

fn named_index(keys: Document, name: &str) -> IndexModel {
    IndexModel::builder()
        .keys(keys)
        .options(IndexOptions::builder().name(name.to_string()).build())
        .build()
}

async fn migration_ids(
    collection: &Collection<MigrationIdDoc>,
    index: &str,
    limit: usize,
) -> Result<Vec<String>, StoreError> {
    let mut cursor = collection
        .find(doc! { "auth_schema_version": { "$ne": AUTH_SCHEMA_VERSION } })
        .projection(doc! { "_id": 1 })
        .hint(Hint::Name(index.to_string()))
        .limit(mongo_limit(limit))
        .await
        .map_err(|error| read_failure("list legacy authentication records", &error))?;
    let mut ids = Vec::with_capacity(limit);
    while let Some(stored) = cursor
        .try_next()
        .await
        .map_err(|error| read_failure("list legacy authentication records cursor", &error))?
    {
        ids.push(stored.id);
    }
    Ok(ids)
}

fn subject_migration_update(
    retention_millis: i64,
    incarnation: &str,
    sweep_sequence: i64,
) -> Vec<Document> {
    vec![doc! {
        "$set": {
            "delete_after": {
                "$cond": [
                    { "$ne": [{ "$type": "$delete_after" }, "missing"] },
                    "$delete_after",
                    {
                        "$let": {
                            "vars": {
                                "retained": {
                                    "$add": [
                                        { "$ifNull": ["$tokens.expires_at", "$$NOW"] },
                                        retention_millis,
                                    ]
                                }
                            },
                            "in": {
                                "$cond": [
                                    {
                                        "$gt": [
                                            {
                                                "$ifNull": [
                                                    "$refresh_lease.expires_at",
                                                    "$$retained",
                                                ]
                                            },
                                            "$$retained",
                                        ]
                                    },
                                    "$refresh_lease.expires_at",
                                    "$$retained",
                                ]
                            }
                        }
                    },
                ]
            },
            // A compatibility rewrite is a new record lifecycle. Fresh fences
            // prevent a pre-migration refresh from committing over this state.
            "incarnation": incarnation,
            "token_generation": { "$add": ["$revision", 1_i64] },
            "sweep_sequence": sweep_sequence,
            "auth_schema_version": AUTH_SCHEMA_VERSION,
        }
    }]
}

fn credential_migration_update(retention_millis: i64, sweep_sequence: i64) -> Vec<Document> {
    vec![doc! {
        "$set": {
            "revoke_debt": {
                "$sortArray": {
                    "input": { "$ifNull": ["$revoke_debt", []] },
                    "sortBy": { "id": 1 },
                }
            },
            "revoke_debt_ordered": true,
            "delete_after": {
                "$cond": [
                    { "$ne": [{ "$type": "$delete_after" }, "missing"] },
                    "$delete_after",
                    {
                        "$cond": [
                            {
                                "$and": [
                                    { "$eq": ["$retired", true] },
                                    { "$eq": [{ "$ifNull": ["$current", Bson::Null] }, Bson::Null] },
                                    { "$eq": [{ "$ifNull": ["$pending", Bson::Null] }, Bson::Null] },
                                    { "$eq": [{ "$ifNull": ["$lease", Bson::Null] }, Bson::Null] },
                                    { "$eq": [{ "$size": { "$ifNull": ["$revoke_debt", []] } }, 0] },
                                ]
                            },
                            { "$add": ["$$NOW", retention_millis] },
                            "$$REMOVE",
                        ]
                    },
                ]
            },
            "sweep_sequence": sweep_sequence,
            "auth_schema_version": AUTH_SCHEMA_VERSION,
        }
    }]
}

fn subject_owner_filter(after: Option<&SubjectOwnerCursor>, through_sequence: u64) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "tokens": { "$type": "object" },
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "sweep_sequence": { "$gt": sequence } },
                doc! {
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.subject.as_str() },
                },
            ],
        );
    }
    filter
}

fn active_credential_reconciliation_filter(
    after: Option<&CredentialReconciliationCursor>,
    through_sequence: u64,
) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "retired": false,
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "sweep_sequence": { "$gt": sequence } },
                doc! {
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.session.as_str() },
                },
            ],
        );
    }
    filter
}

fn subject_owner_hint() -> Hint {
    Hint::Name(SUBJECT_OWNER_INDEX.to_string())
}

fn unleased_subject_refresh_filter(
    refresh_at: SystemTime,
    after: Option<&SubjectRefreshCursor>,
    through_sequence: u64,
) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "tokens.refresh_token": { "$type": "object" },
        "refresh_lease": Bson::Null,
        "tokens.expires_at": {
            "$type": "date",
            "$lte": DateTime::from_system_time(refresh_at),
        },
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let ready_at = DateTime::from_system_time(after.ready_at);
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "tokens.expires_at": { "$gt": ready_at } },
                doc! {
                    "tokens.expires_at": ready_at,
                    "sweep_sequence": { "$gt": sequence },
                },
                doc! {
                    "tokens.expires_at": ready_at,
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.subject.as_str() },
                },
            ],
        );
    }
    filter
}

fn expired_subject_refresh_lease_filter(
    now: SystemTime,
    after: Option<&SubjectRefreshCursor>,
    through_sequence: u64,
) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "tokens.refresh_token": { "$type": "object" },
        "refresh_lease.expires_at": {
            "$type": "date",
            "$lte": DateTime::from_system_time(now),
        },
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let ready_at = DateTime::from_system_time(after.ready_at);
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "refresh_lease.expires_at": { "$gt": ready_at } },
                doc! {
                    "refresh_lease.expires_at": ready_at,
                    "sweep_sequence": { "$gt": sequence },
                },
                doc! {
                    "refresh_lease.expires_at": ready_at,
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.subject.as_str() },
                },
            ],
        );
    }
    filter
}

fn current_credential_reissue_filter(
    issued_before: SystemTime,
    after: Option<&CredentialReissueCursor>,
    through_sequence: u64,
) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "retired": false,
        "lease": Bson::Null,
        "pending": Bson::Null,
        "current.issued_at": {
            "$type": "date",
            "$lte": DateTime::from_system_time(issued_before),
        },
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let issued_at = DateTime::from_system_time(after.issued_at);
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "current.issued_at": { "$gt": issued_at } },
                doc! {
                    "current.issued_at": issued_at,
                    "sweep_sequence": { "$gt": sequence },
                },
                doc! {
                    "current.issued_at": issued_at,
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.session.as_str() },
                },
            ],
        );
    }
    filter
}

fn pending_credential_reissue_filter(
    after: Option<&CredentialMutationCursor>,
    through_sequence: u64,
) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "retired": false,
        "lease": Bson::Null,
        "pending.issued_at": { "$type": "date" },
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "sweep_sequence": { "$gt": sequence } },
                doc! {
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.session.as_str() },
                },
            ],
        );
    }
    filter
}

fn expired_credential_mint_lease_filter(
    now: SystemTime,
    after: Option<&CredentialLeaseCursor>,
    through_sequence: u64,
) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "retired": false,
        "pending": Bson::Null,
        "current.issued_at": { "$type": "date" },
        "lease.expires_at": {
            "$type": "date",
            "$lte": DateTime::from_system_time(now),
        },
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let ready_at = DateTime::from_system_time(after.ready_at);
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "lease.expires_at": { "$gt": ready_at } },
                doc! {
                    "lease.expires_at": ready_at,
                    "sweep_sequence": { "$gt": sequence },
                },
                doc! {
                    "lease.expires_at": ready_at,
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.session.as_str() },
                },
            ],
        );
    }
    filter
}

fn credential_debt_parent_filter(
    after: Option<&RevokeDebtCursor>,
    through_sequence: u64,
) -> Document {
    let mut filter = doc! {
        "_id": { "$type": "string" },
        "revoke_debt.0.id": { "$type": "string" },
        "revoke_debt_ordered": true,
        "sweep_sequence": {
            "$type": "long",
            "$lte": mongo_sweep_sequence(through_sequence),
        },
    };
    if let Some(after) = after {
        let sequence = mongo_sweep_sequence(after.sweep_sequence);
        filter.insert(
            "$or",
            vec![
                doc! { "sweep_sequence": { "$gt": sequence } },
                doc! {
                    "sweep_sequence": sequence,
                    "_id": { "$gt": after.session.as_str() },
                },
            ],
        );
    }
    filter
}

fn revoke_debt_after_filter(after: &RevokeDebtCursor, through_sequence: u64) -> Document {
    doc! {
        "_id": after.session.as_str(),
        "sweep_sequence": {
            "$eq": mongo_sweep_sequence(after.sweep_sequence),
            "$lte": mongo_sweep_sequence(through_sequence),
        },
        "revoke_debt.0": { "$exists": true },
        "revoke_debt_ordered": true,
        "revoke_debt": {
            "$elemMatch": { "id": { "$type": "string", "$gt": &after.credential_id } },
        },
    }
}

fn unordered_revoke_debt_filter() -> Document {
    doc! {
        "revoke_debt.0": { "$exists": true },
        "revoke_debt_ordered": { "$ne": true },
    }
}

fn revoke_debt_order_update(sweep_sequence: i64) -> Vec<Document> {
    vec![doc! {
        "$set": {
            "revoke_debt": {
                "$sortArray": {
                    "input": { "$ifNull": ["$revoke_debt", []] },
                    "sortBy": { "id": 1 },
                }
            },
            "revoke_debt_ordered": true,
            "sweep_sequence": sweep_sequence,
        }
    }]
}

fn revoke_debt_after_projection(after_id: &str) -> Document {
    doc! {
        "_id": 1,
        "sweep_sequence": 1,
        "revoke_debt": {
            "$elemMatch": { "id": { "$gt": after_id } },
        },
    }
}

fn first_revoke_debt_projection() -> Document {
    doc! { "_id": 1, "sweep_sequence": 1, "revoke_debt": { "$slice": 1 } }
}

#[cfg(test)]
fn last_revoke_debt_projection() -> Document {
    doc! { "_id": 1, "sweep_sequence": 1, "revoke_debt": { "$slice": -1 } }
}

fn revoke_debt_page_work(mut stored: RevokeDebtPageDoc) -> Result<RevokeDebtWork, StoreError> {
    if stored.revoke_debt.len() != 1 {
        return Err(StoreError::Malformed(
            "projected credential revoke-debt page must contain exactly one entry".into(),
        ));
    }
    let debt = stored.revoke_debt.pop().unwrap();
    Ok(RevokeDebtWork {
        session: SessionId(stored.id),
        debt: RevokeDebt {
            id: debt.id,
            owner: SubjectId(debt.owner),
            queued_at: debt.queued_at.to_system_time(),
        },
    })
}

fn revoke_debt_page_item(
    stored: Document,
) -> Result<AuthSweepItem<RevokeDebtCursor, RevokeDebtWork>, StoreError> {
    let cursor = revoke_debt_cursor_from_document(&stored)?;
    let state = mongodb::bson::deserialize_from_document::<RevokeDebtPageDoc>(stored)
        .map_err(|_| StoreError::Malformed("stored credential revoke debt is malformed".into()))
        .and_then(revoke_debt_page_work);
    Ok(AuthSweepItem { cursor, state })
}

fn mongo_sweep_sequence(sequence: u64) -> i64 {
    i64::try_from(sequence).unwrap_or(i64::MAX)
}

fn mongo_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
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

fn document_string(stored: &Document, field: &str, record: &str) -> Result<String, StoreError> {
    stored
        .get_str(field)
        .map(str::to_string)
        .map_err(|_| StoreError::Malformed(format!("stored {record} {field} must be a string")))
}

fn document_sequence(stored: &Document, record: &str) -> Result<u64, StoreError> {
    let sequence = stored.get_i64("sweep_sequence").map_err(|_| {
        StoreError::Malformed(format!(
            "stored {record} sweep_sequence must be a 64-bit integer"
        ))
    })?;
    read_counter(sequence, &format!("{record} sweep sequence"))
}

fn nested_document_time(
    stored: &Document,
    container: &str,
    field: &str,
    record: &str,
) -> Result<SystemTime, StoreError> {
    stored
        .get_document(container)
        .and_then(|nested| nested.get_datetime(field))
        .map(|value| value.to_system_time())
        .map_err(|_| {
            StoreError::Malformed(format!(
                "stored {record} {container}.{field} must be a date"
            ))
        })
}

fn subject_refresh_cursor_from_document(
    stored: &Document,
    container: &str,
    field: &str,
) -> Result<SubjectRefreshCursor, StoreError> {
    Ok(SubjectRefreshCursor {
        ready_at: nested_document_time(stored, container, field, "subject authentication")?,
        sweep_sequence: document_sequence(stored, "subject authentication")?,
        subject: SubjectId(document_string(stored, "_id", "subject authentication")?),
    })
}

fn subject_owner_cursor_from_document(stored: &Document) -> Result<SubjectOwnerCursor, StoreError> {
    Ok(SubjectOwnerCursor {
        sweep_sequence: document_sequence(stored, "subject authentication")?,
        subject: SubjectId(document_string(stored, "_id", "subject authentication")?),
    })
}

fn credential_reissue_cursor_from_document(
    stored: &Document,
) -> Result<CredentialReissueCursor, StoreError> {
    Ok(CredentialReissueCursor {
        issued_at: nested_document_time(stored, "current", "issued_at", "session credential")?,
        sweep_sequence: document_sequence(stored, "session credential")?,
        session: SessionId(document_string(stored, "_id", "session credential")?),
    })
}

fn credential_lease_cursor_from_document(
    stored: &Document,
) -> Result<CredentialLeaseCursor, StoreError> {
    Ok(CredentialLeaseCursor {
        ready_at: nested_document_time(stored, "lease", "expires_at", "session credential")?,
        sweep_sequence: document_sequence(stored, "session credential")?,
        session: SessionId(document_string(stored, "_id", "session credential")?),
    })
}

fn credential_mutation_cursor_from_document(
    stored: &Document,
) -> Result<CredentialMutationCursor, StoreError> {
    Ok(CredentialMutationCursor {
        sweep_sequence: document_sequence(stored, "session credential")?,
        session: SessionId(document_string(stored, "_id", "session credential")?),
    })
}

fn credential_reconciliation_cursor_from_document(
    stored: &Document,
) -> Result<CredentialReconciliationCursor, StoreError> {
    Ok(CredentialReconciliationCursor {
        sweep_sequence: document_sequence(stored, "session credential")?,
        session: SessionId(document_string(stored, "_id", "session credential")?),
    })
}

fn revoke_debt_cursor_from_document(stored: &Document) -> Result<RevokeDebtCursor, StoreError> {
    let debts = stored.get_array("revoke_debt").map_err(|_| {
        StoreError::Malformed("projected credential revoke debt must be an array".into())
    })?;
    if debts.len() != 1 {
        return Err(StoreError::Malformed(
            "projected credential revoke-debt page must contain exactly one entry".into(),
        ));
    }
    let credential_id = debts[0]
        .as_document()
        .and_then(|debt| debt.get_str("id").ok())
        .map(str::to_string)
        .ok_or_else(|| {
            StoreError::Malformed("stored credential revoke-debt id must be a string".into())
        })?;
    Ok(RevokeDebtCursor {
        sweep_sequence: document_sequence(stored, "session credential")?,
        session: SessionId(document_string(stored, "_id", "session credential")?),
        credential_id,
    })
}

fn decode_subject_document(
    stored: Document,
    cipher: &AuthCipher,
) -> Result<SubjectAuthState, StoreError> {
    let stored =
        mongodb::bson::deserialize_from_document::<SubjectAuthDoc>(stored).map_err(|_| {
            StoreError::Malformed("stored subject authentication document is malformed".into())
        })?;
    subject_from_doc(stored, cipher)
}

fn decode_credential_document(
    stored: Document,
    cipher: &AuthCipher,
) -> Result<SessionCredentialState, StoreError> {
    let stored =
        mongodb::bson::deserialize_from_document::<SessionCredentialDoc>(stored).map_err(|_| {
            StoreError::Malformed("stored session credential document is malformed".into())
        })?;
    credential_state_from_doc(stored, cipher)
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

fn subject_delete_after(
    state: &SubjectAuthState,
    now: SystemTime,
) -> Result<SystemTime, StoreError> {
    let anchor = state
        .tokens
        .as_ref()
        .map_or(now, |tokens| tokens.expires_at);
    let retained = anchor.checked_add(SUBJECT_RETENTION).ok_or_else(|| {
        StoreError::Malformed("subject retention deadline exceeds the system clock".into())
    })?;
    Ok(state
        .refresh_lease
        .as_ref()
        .map_or(retained, |lease| retained.max(lease.expires_at)))
}

fn subject_to_doc(
    state: SubjectAuthState,
    cipher: &AuthCipher,
) -> Result<SubjectAuthDoc, StoreError> {
    let delete_after = subject_delete_after(&state, SystemTime::now())?;
    let SubjectAuthState {
        subject,
        incarnation,
        revision,
        token_generation,
        tokens,
        refresh_lease,
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
        incarnation,
        revision: stored_counter(revision, "subject auth revision")?,
        token_generation: stored_counter(token_generation, "subject token generation")?,
        sweep_sequence: 0,
        tokens,
        refresh_lease: refresh_lease.map(|lease| SubjectRefreshLeaseDoc {
            holder: lease.holder,
            expires_at: DateTime::from_system_time(lease.expires_at),
        }),
        delete_after: Some(DateTime::from_system_time(delete_after)),
        auth_schema_version: AUTH_SCHEMA_VERSION,
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
    let _retention_deadline = stored.delete_after;
    let _schema_version = stored.auth_schema_version;
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
        incarnation: stored.incarnation,
        revision: read_counter(stored.revision, "subject auth revision")?,
        token_generation: read_counter(stored.token_generation, "subject token generation")?,
        tokens,
        refresh_lease: stored.refresh_lease.map(|lease| SubjectRefreshLease {
            holder: lease.holder,
            expires_at: lease.expires_at.to_system_time(),
        }),
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
    let delete_after = state
        .is_settled_retired()
        .then(|| DateTime::from_system_time(SystemTime::now() + SETTLED_CREDENTIAL_RETENTION));
    let SessionCredentialState {
        session,
        generation,
        current,
        pending,
        mut revoke_debt,
        lease,
        retired,
    } = state;
    let id = session.0;
    revoke_debt.sort_by(|left, right| left.id.cmp(&right.id));
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
        revoke_debt_ordered: true,
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
        sweep_sequence: 0,
        retired,
        delete_after,
        auth_schema_version: AUTH_SCHEMA_VERSION,
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
    let _retention_deadline = stored.delete_after;
    let _schema_version = stored.auth_schema_version;
    let id = stored.id;
    let mut revoke_debt = stored
        .revoke_debt
        .into_iter()
        .map(|debt| RevokeDebt {
            id: debt.id,
            owner: SubjectId(debt.owner),
            queued_at: debt.queued_at.to_system_time(),
        })
        .collect::<Vec<_>>();
    if !stored.revoke_debt_ordered {
        revoke_debt.sort_by(|left, right| left.id.cmp(&right.id));
    }
    Ok(SessionCredentialState {
        current: stored
            .current
            .map(|value| credential_from_doc(value, cipher, &id, CURRENT_VALUE_FIELD))
            .transpose()?,
        pending: stored
            .pending
            .map(|value| credential_from_doc(value, cipher, &id, PENDING_VALUE_FIELD))
            .transpose()?,
        revoke_debt,
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
