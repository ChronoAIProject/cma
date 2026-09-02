//! Sponsorship authority, accounting, encrypted provider material, and audit in MongoDB.

use std::collections::BTreeSet;
use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use futures_util::{future::BoxFuture, FutureExt as _, TryStreamExt as _};
use layer2_core::{
    BeginProviderGrantIssue, ClaimGrantActivation, ClaimGrantActivationCleanup,
    ClaimInvalidatedGrantActivation, ClaimSponsoredUsageDispatch, CommitGrantActivation,
    CompleteGrantActivationCleanup, CompleteInvalidatedGrantActivation, CompletedProviderOperation,
    CompletedProviderOperationOutcome, GrantActivationClaim, GrantActivationDenial,
    GrantActivationIntent, GrantActivationLease, GrantActivationState, GrantCapacityRelease,
    InertGrantCapability, Launch, LaunchId, Page, PageCursor, PageRequest,
    PendingProviderOperation, PersistSponsoredAcceptance, PersistSponsorshipPolicy,
    ProviderCredentialAuthority, ProviderCredentialSecret, ProviderGrantAuthority,
    ProviderGrantReceipt, ProviderOperationKind, ProviderOperationPhase, ReconcileSponsoredUsage,
    ReservationDisposition, ReserveSponsoredUsage, SensitiveProviderValue, SessionId,
    SponsorBillingOwner, SponsoredAuditAction, SponsoredAuditEvent, SponsoredCredentialId,
    SponsoredCredentialMaterialState, SponsoredCredentialRecord,
    SponsoredCredentialRegistrationIdentity, SponsoredCredentialState, SponsoredGrant,
    SponsoredGrantAcceptance, SponsoredGrantId, SponsoredGrantState, SponsoredGrantTerms,
    SponsoredLaunchAuthority, SponsoredLaunchState, SponsoredModelDispatchPayload,
    SponsoredModelRecoveryRecord, SponsoredModelResponsePayload, SponsoredUsageDispatchClaim,
    SponsoredUsageDispatchLease, SponsoredUsageReconciliationFence, SponsoredUsageReservation,
    SponsoredUsageReservationId, SponsoredUsageReservationOutcome, SponsoredUsageState,
    SponsoredUsageSummary, SponsorshipCiphertextRecordKind, SponsorshipCiphertextRewrapCandidate,
    SponsorshipGateFence, SponsorshipOperationalPseudonymSet, SponsorshipOperationalPseudonymizer,
    SponsorshipPolicy, SponsorshipPolicyCreationIdentity, SponsorshipPolicyId,
    SponsorshipPolicyUsage, SponsorshipRecoveryPage, SponsorshipRecoveryPageRequest,
    SponsorshipRuntimeLimits, SponsorshipStore, SponsorshipStoreError, StoredSponsoredCredential,
    StoredSponsorshipPolicy, SubjectId, TriggerRevisionId, TriggerStoreError,
    MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS, MAX_SPONSORED_AUDIT_PAGE,
    SPONSORED_MODEL_DISPATCH_PAYLOAD_SCHEMA_VERSION,
    SPONSORED_MODEL_RESPONSE_PAYLOAD_SCHEMA_VERSION,
};
use mongodb::bson::{doc, Bson, DateTime, Document};
use mongodb::error::{Error, ErrorKind, WriteFailure};
use mongodb::options::{Hint, IndexOptions, ReadConcern, ReturnDocument, WriteConcern};
use mongodb::{Client, ClientSession, Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::durability::majority_collection;
use super::sponsorship_encryption::{
    validate_independent_keyrings, validate_key_id, SponsorshipCipher, SponsorshipCipherEnvelope,
    SponsorshipKeyring, SponsorshipOperationalKeyring,
};
use super::trigger::{read_launch, LaunchDoc};

const DOCUMENT_SCHEMA_VERSION: i32 = 1;
const CREDENTIAL_REGISTRATION_DOCUMENT_SCHEMA_VERSION: i32 = 2;
const POLICY_CREATION_DOCUMENT_SCHEMA_VERSION: i32 = 1;
const GATE_ID: &str = "sponsorship-gate-v1";
const GATE: &str = "sponsorship_gate";
const CREDENTIALS: &str = "sponsorship_credentials";
const CREDENTIAL_REGISTRATIONS: &str = "sponsorship_credential_registrations";
const CREDENTIAL_OPERATIONS: &str = "sponsorship_credential_operations";
const POLICY_CREATIONS: &str = "sponsorship_policy_creations";
const POLICIES: &str = "sponsorship_policies";
const POLICY_COUNTERS: &str = "sponsorship_policy_counters";
const SPONSOR_COUNTERS: &str = "sponsorship_sponsor_counters";
const SPONSOR_LAUNCH_ADMISSIONS: &str = "sponsorship_sponsor_launch_admissions";
const ACCEPTANCES: &str = "sponsorship_acceptances";
const ACTIVATIONS: &str = "sponsorship_grant_activations";
const GRANTS: &str = "sponsorship_grants";
const RESERVATIONS: &str = "sponsorship_usage_reservations";
const AUDITS: &str = "sponsorship_audit_events";
const TRIGGER_LAUNCHES: &str = "trigger_launches";
const DUPLICATE_KEY: i32 = 11_000;
const ACTIVATION_LEASE_MILLIS: i64 = 30_000;
const USAGE_DISPATCH_LEASE_MILLIS: i64 = 30_000;
const RATE_WINDOW_MILLIS: i64 = 60_000;
const MAX_KILL_SWITCH_INVALIDATIONS: i64 = 1_000;
const MAX_USAGE_REWRAP_PAGE: u32 = 4;
const MAX_SPONSORSHIP_RECOVERY_CURSOR_BYTES: usize = 2_048;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GateDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    configuration_digest: String,
    epoch: i64,
    kill_switch_engaged: bool,
    trusted_provider_policy_key: String,
    runtime_limits: SponsorshipRuntimeLimits,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kill_switch_engagement_id: Option<String>,
    #[serde(default)]
    kill_switch_drain_complete: bool,
    #[serde(default)]
    authority_read_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ProviderOperationDoc {
    kind: String,
    phase: String,
    provider_request_id: String,
    client_request_id_digest: String,
    payload_digest: String,
    requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CompletedProviderOperationDoc {
    kind: String,
    provider_request_id: String,
    client_request_id_digest: String,
    payload_digest: String,
    outcome: String,
    completed_at: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct CredentialDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    owner: SponsorBillingOwner,
    owner_subject: String,
    provider: String,
    provider_policy_key: String,
    provider_policy_handle: SponsorshipCipherEnvelope,
    provider_credential_id_digest: String,
    #[serde(default)]
    provider_credential_id_aliases: Vec<String>,
    provider_state_version: i64,
    ownership_verified_at: String,
    supports_expiry: bool,
    supports_revocation: bool,
    state: String,
    material_state: String,
    generation: i64,
    created_at: String,
    updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_operation: Option<ProviderOperationDoc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    superseded_operation: Option<ProviderOperationDoc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_completed_operation: Option<CompletedProviderOperationDoc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_secret: Option<SponsorshipCipherEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_secret: Option<SponsorshipCipherEnvelope>,
    encryption_key_id: String,
    #[serde(default)]
    authority_read_revision: i64,
}

impl std::fmt::Debug for CredentialDoc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CredentialDoc")
            .field("id", &self.id)
            .field("document_schema_version", &self.document_schema_version)
            .field("owner_subject", &self.owner_subject)
            .field("provider", &self.provider)
            .field("provider_policy_key", &self.provider_policy_key)
            .field(
                "provider_credential_id_digest",
                &self.provider_credential_id_digest,
            )
            .field("state", &self.state)
            .field("material_state", &self.material_state)
            .field("generation", &self.generation)
            .field("encryption_key_id", &self.encryption_key_id)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialRegistrationDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    request_aliases: Vec<String>,
    command_aliases: Vec<String>,
    credential_id: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PolicyCreationDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    request_aliases: Vec<String>,
    command_aliases: Vec<String>,
    payload_digest: String,
    policy_id: String,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialOperationLedgerId {
    credential_id: String,
    operation_kind: String,
    client_request_id_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CredentialOperationLedgerDoc {
    #[serde(rename = "_id")]
    id: CredentialOperationLedgerId,
    document_schema_version: i32,
    operation: CompletedProviderOperationDoc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PolicyUsageDoc {
    consumed: layer2_core::SponsoredUsageSummary,
    reserved: layer2_core::SponsoredUsageSummary,
    active_launches: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PolicyDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    owner_subject: String,
    sponsor_key: String,
    credential_id: String,
    state: String,
    expires_at: String,
    expires_at_database: DateTime,
    created_at: String,
    updated_at: String,
    policy: SponsorshipPolicy,
    usage: PolicyUsageDoc,
    #[serde(default)]
    authority_read_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PolicyCounterDoc {
    #[serde(rename = "_id")]
    policy_id: String,
    document_schema_version: i32,
    rate_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SponsorCounterDoc {
    #[serde(rename = "_id")]
    sponsor_key: String,
    document_schema_version: i32,
    policy_revision: i64,
    launch_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SponsorLaunchAdmissionDoc {
    #[serde(rename = "_id")]
    launch_id: String,
    document_schema_version: i32,
    sponsor_key: String,
    admitted_at_database: DateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AcceptanceDoc {
    #[serde(rename = "_id")]
    launch_id: String,
    document_schema_version: i32,
    idempotency_aliases: Vec<String>,
    contributor: String,
    policy_id: String,
    accepted_at: String,
    acceptance: SponsoredGrantAcceptance,
    #[serde(default)]
    authority_read_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GateFenceDoc {
    configuration_digest: String,
    epoch: i64,
    kill_switch_engaged: bool,
    trusted_provider_policy_key: String,
    runtime_limits: SponsorshipRuntimeLimits,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ProviderReceiptDoc {
    provider_grant_id: SponsorshipCipherEnvelope,
    provider_state_version: i64,
    expires_at: String,
    revocation_handle: SponsorshipCipherEnvelope,
    issued_at: String,
}

impl std::fmt::Debug for ProviderReceiptDoc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProviderReceiptDoc")
            .field("provider_grant_id", &"<encrypted>")
            .field("provider_state_version", &self.provider_state_version)
            .field("expires_at", &self.expires_at)
            .field("revocation_handle", &"<encrypted>")
            .field("issued_at", &self.issued_at)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActivationLeaseDoc {
    holder: String,
    acquired_at: String,
    expires_at: String,
    expires_at_database: DateTime,
}

#[derive(Clone, Serialize, Deserialize)]
struct ActivationDoc {
    #[serde(rename = "_id")]
    launch_id: String,
    document_schema_version: i32,
    session_id: String,
    policy_id: String,
    policy_generation: i64,
    credential_generation: i64,
    launch_authority_digest: String,
    acceptance_digest: String,
    gate_fence: GateFenceDoc,
    allocation: layer2_core::SponsoredUsageSummary,
    request_fingerprint: String,
    billing_request_id: String,
    provider_request_id: String,
    state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    denial: Option<String>,
    capacity_held: bool,
    revision: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lease: Option<ActivationLeaseDoc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_receipt: Option<ProviderReceiptDoc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cleanup_operation: Option<ProviderOperationDoc>,
    created_at: String,
    updated_at: String,
    encryption_key_id: String,
}

impl std::fmt::Debug for ActivationDoc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActivationDoc")
            .field("launch_id", &self.launch_id)
            .field("policy_id", &self.policy_id)
            .field("state", &self.state)
            .field("revision", &self.revision)
            .field("lease", &self.lease)
            .field("provider_receipt", &self.provider_receipt)
            .field("encryption_key_id", &self.encryption_key_id)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct GrantDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    schema_version: i64,
    tenant_id: String,
    policy_id: String,
    policy_generation: i64,
    credential_generation: i64,
    trigger_revision_id: String,
    trigger_revision_digest: String,
    launch_id: String,
    session_id: String,
    contributor: String,
    sponsor_subject: String,
    terms_digest: String,
    acceptance: SponsoredGrantAcceptance,
    authority: ProviderGrantAuthority,
    capability: InertGrantCapability,
    state: String,
    allocation: layer2_core::SponsoredUsageSummary,
    usage: layer2_core::SponsoredUsageSummary,
    unresolved_usage: layer2_core::SponsoredUsageSummary,
    generation: i64,
    created_at: String,
    updated_at: String,
    expires_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_revocation: Option<ProviderOperationDoc>,
    provider_receipt: ProviderReceiptDoc,
    encryption_key_id: String,
    #[serde(default)]
    authority_read_revision: i64,
}

impl std::fmt::Debug for GrantDoc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GrantDoc")
            .field("id", &self.id)
            .field("policy_id", &self.policy_id)
            .field("launch_id", &self.launch_id)
            .field("contributor", &self.contributor)
            .field("state", &self.state)
            .field("generation", &self.generation)
            .field("provider_receipt", &self.provider_receipt)
            .field("encryption_key_id", &self.encryption_key_id)
            .finish()
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct ReservationDoc {
    #[serde(rename = "_id")]
    reservation_id: String,
    document_schema_version: i32,
    request_aliases: Vec<String>,
    grant_id: String,
    policy_id: String,
    state: String,
    created_at: String,
    updated_at: String,
    rate_observed_at: DateTime,
    reservation: SponsoredUsageReservation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credential_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_dispatch_schema_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_dispatch_request_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_dispatch_payload: Option<SponsorshipCipherEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_response_schema_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_response_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model_response_payload: Option<SponsorshipCipherEnvelope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    encryption_key_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ReservationIdDoc {
    #[serde(rename = "_id")]
    reservation_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ReservationRecoveryKeyDoc {
    #[serde(rename = "_id")]
    reservation_id: String,
    updated_at: String,
}

impl std::fmt::Debug for ReservationDoc {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReservationDoc")
            .field("reservation_id", &self.reservation_id)
            .field("grant_id", &self.grant_id)
            .field("policy_id", &self.policy_id)
            .field("state", &self.state)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field(
                "model_dispatch_payload",
                &self.model_dispatch_payload.as_ref().map(|_| "<encrypted>"),
            )
            .field(
                "model_response_payload",
                &self.model_response_payload.as_ref().map(|_| "<encrypted>"),
            )
            .field("encryption_key_id", &self.encryption_key_id)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditDoc {
    #[serde(rename = "_id")]
    event_id: String,
    document_schema_version: i32,
    actor_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    grant_id: Option<String>,
    occurred_at: String,
    event: SponsoredAuditEvent,
}

#[derive(Clone)]
pub struct MongoSponsorshipStore {
    client: Client,
    cipher: Arc<SponsorshipCipher>,
    operational: Arc<SponsorshipOperationalPseudonymizer>,
    gate: Collection<GateDoc>,
    credentials: Collection<CredentialDoc>,
    credential_registrations: Collection<CredentialRegistrationDoc>,
    credential_operations: Collection<CredentialOperationLedgerDoc>,
    policy_creations: Collection<PolicyCreationDoc>,
    policies: Collection<PolicyDoc>,
    policy_counters: Collection<PolicyCounterDoc>,
    sponsor_counters: Collection<SponsorCounterDoc>,
    sponsor_launch_admissions: Collection<SponsorLaunchAdmissionDoc>,
    acceptances: Collection<AcceptanceDoc>,
    activations: Collection<ActivationDoc>,
    grants: Collection<GrantDoc>,
    reservations: Collection<ReservationDoc>,
    audits: Collection<AuditDoc>,
    trigger_launches: Collection<LaunchDoc>,
}

impl MongoSponsorshipStore {
    /// Open the sponsorship authority only on a transaction-capable Mongo topology.
    pub(crate) async fn open(
        client: &Client,
        database_name: &str,
        deployment: &str,
        keyring: SponsorshipKeyring,
        operational_keyring: SponsorshipOperationalKeyring,
        initial_gate: SponsorshipGateFence,
    ) -> Result<Self, SponsorshipStoreError> {
        validate_gate(&initial_gate)?;
        validate_independent_keyrings(&keyring, &operational_keyring)
            .map_err(SponsorshipStoreError::Malformed)?;
        let operational = Arc::new(operational_keyring.pseudonymizer(deployment)?);
        let cipher = Arc::new(SponsorshipCipher::new(deployment, keyring)?);
        let database = client.database(database_name);
        validate_transaction_topology(&database).await?;
        let store = Self {
            client: client.clone(),
            cipher,
            operational,
            gate: majority_collection(&database, GATE),
            credentials: majority_collection(&database, CREDENTIALS),
            credential_registrations: majority_collection(&database, CREDENTIAL_REGISTRATIONS),
            credential_operations: majority_collection(&database, CREDENTIAL_OPERATIONS),
            policy_creations: majority_collection(&database, POLICY_CREATIONS),
            policies: majority_collection(&database, POLICIES),
            policy_counters: majority_collection(&database, POLICY_COUNTERS),
            sponsor_counters: majority_collection(&database, SPONSOR_COUNTERS),
            sponsor_launch_admissions: majority_collection(&database, SPONSOR_LAUNCH_ADMISSIONS),
            acceptances: majority_collection(&database, ACCEPTANCES),
            activations: majority_collection(&database, ACTIVATIONS),
            grants: majority_collection(&database, GRANTS),
            reservations: majority_collection(&database, RESERVATIONS),
            audits: majority_collection(&database, AUDITS),
            trigger_launches: majority_collection(&database, TRIGGER_LAUNCHES),
        };
        store.validate_operational_dependencies().await?;
        store.initialize_gate(&initial_gate).await?;
        store.probe_transaction_capability().await?;
        store.ensure_indexes().await?;
        match store.resume_kill_switch_drain().await {
            Ok(true) => {}
            Ok(false) => tracing::warn!(
                "sponsorship kill-switch drain remains incomplete after one bounded startup batch"
            ),
            Err(error) => {
                tracing::warn!(%error, "sponsorship kill-switch drain will resume on the next retry")
            }
        }
        tracing::info!(
            sponsorship_encryption_key_id = store.cipher.current_key_id(),
            sponsorship_operational_key_id = store.operational.current_key_id(),
            "Mongo sponsorship authority ready"
        );
        Ok(store)
    }

    async fn probe_transaction_capability(&self) -> Result<(), SponsorshipStoreError> {
        let mut session = self.client.start_session().await.map_err(|error| {
            unavailable("start sponsorship transaction capability probe", error)
        })?;
        let probe_id = format!("sponsorship-transaction-probe-{}", uuid::Uuid::new_v4());
        let gate = self.gate.clone_with_type::<Document>();
        let result = session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run((gate, probe_id), |session, (gate, probe_id)| {
                async move {
                    gate.find_one(doc! { "_id": GATE_ID })
                        .session(&mut *session)
                        .await?;
                    gate.insert_one(doc! {
                        "_id": &*probe_id,
                        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                        "kind": "transaction_capability_probe",
                    })
                    .session(&mut *session)
                    .await?;
                    let deleted = gate
                        .delete_one(doc! { "_id": &*probe_id })
                        .session(&mut *session)
                        .await?;
                    if deleted.deleted_count != 1 {
                        return Err(custom(SponsorshipStoreError::Unavailable(
                            "sponsorship transaction probe could not remove its temporary row"
                                .into(),
                        )));
                    }
                    Ok(())
                }
                .boxed()
            })
            .await
            .map_err(|error| unavailable("commit sponsorship transaction capability probe", error));
        result
    }

    async fn initialize_gate(
        &self,
        initial: &SponsorshipGateFence,
    ) -> Result<(), SponsorshipStoreError> {
        let candidate = gate_doc(initial)?;
        match self.gate.insert_one(&candidate).await {
            Ok(_) => Ok(()),
            Err(error) if is_duplicate_key(&error) => {
                let stored = self
                    .gate
                    .find_one(doc! { "_id": GATE_ID })
                    .await
                    .map_err(|error| stored_read_failure("read sponsorship gate", error))?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Unavailable(
                            "sponsorship gate disappeared during initialization".into(),
                        )
                    })?;
                let stored = read_gate(stored)?;
                if !startup_gate_is_compatible(initial, &stored) {
                    return Err(SponsorshipStoreError::GateChanged);
                }
                Ok(())
            }
            Err(error) => Err(unavailable("initialize sponsorship gate", error)),
        }
    }

    async fn ensure_indexes(&self) -> Result<(), SponsorshipStoreError> {
        for index in [
            IndexModel::builder()
                .keys(doc! { "provider": 1, "provider_credential_id_aliases": 1 })
                .options(unique_named(
                    "sponsorship_credential_provider_identity_alias",
                ))
                .build(),
            IndexModel::builder()
                .keys(doc! { "owner_subject": 1, "created_at": -1, "_id": -1 })
                .options(named("sponsorship_credentials_by_owner"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "updated_at": 1, "_id": 1 })
                .options(named("sponsorship_credentials_pending"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "encryption_key_id": 1, "_id": 1 })
                .options(named("sponsorship_credentials_by_encryption_key"))
                .build(),
        ] {
            self.credentials
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship credential index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(doc! { "request_aliases": 1 })
                .options(
                    IndexOptions::builder()
                        .name("sponsorship_registration_by_request_alias".to_string())
                        .unique(true)
                        .build(),
                )
                .build(),
            IndexModel::builder()
                .keys(doc! { "credential_id": 1 })
                .options(named("sponsorship_registration_by_credential"))
                .build(),
        ] {
            self.credential_registrations
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship registration index", error))?;
        }
        self.credential_operations
            .create_index(
                IndexModel::builder()
                    .keys(doc! {
                        "_id.credential_id": 1,
                        "_id.operation_kind": 1,
                        "_id.client_request_id_digest": 1,
                    })
                    .options(unique_named("sponsorship_credential_operation_replay"))
                    .build(),
            )
            .await
            .map_err(|error| unavailable("create credential operation index", error))?;
        self.policy_creations
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "request_aliases": 1 })
                    .options(unique_named("sponsorship_policy_creation_by_request_alias"))
                    .build(),
            )
            .await
            .map_err(|error| unavailable("create sponsorship policy creation index", error))?;
        for index in [
            IndexModel::builder()
                .keys(doc! { "owner_subject": 1, "created_at": -1, "_id": -1 })
                .options(named("sponsorship_policies_by_owner"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "credential_id": 1, "state": 1, "expires_at": 1 })
                .options(named("sponsorship_policies_by_credential"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "expires_at": 1, "_id": 1 })
                .options(named("sponsorship_policies_due"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "sponsor_key": 1, "state": 1, "expires_at_database": 1, "_id": 1 })
                .options(named("sponsorship_active_policies_by_sponsor"))
                .build(),
        ] {
            self.policies
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship policy index", error))?;
        }
        self.sponsor_launch_admissions
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "sponsor_key": 1, "admitted_at_database": -1, "_id": 1 })
                    .options(named("sponsorship_launch_admissions_by_sponsor"))
                    .build(),
            )
            .await
            .map_err(|error| unavailable("create sponsor Launch admission index", error))?;
        for index in [
            IndexModel::builder()
                .keys(doc! { "idempotency_aliases": 1 })
                .options(
                    IndexOptions::builder()
                        .name("sponsorship_acceptance_by_idempotency_alias".to_string())
                        .unique(true)
                        .build(),
                )
                .build(),
            IndexModel::builder()
                .keys(doc! { "policy_id": 1, "accepted_at": -1, "_id": -1 })
                .options(named("sponsorship_acceptances_by_policy"))
                .build(),
        ] {
            self.acceptances
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship acceptance index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(
                    doc! { "state": 1, "lease.expires_at_database": 1, "updated_at": 1, "_id": 1 },
                )
                .options(named("sponsorship_activations_due"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "updated_at": 1, "_id": 1 })
                .options(named("sponsorship_activations_recovery"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "created_at": 1, "_id": 1 })
                .options(named("sponsorship_activations_kill_switch"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "policy_id": 1, "state": 1, "updated_at": 1, "_id": 1 })
                .options(named("sponsorship_activations_by_policy"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "encryption_key_id": 1, "_id": 1 })
                .options(named("sponsorship_activations_by_encryption_key"))
                .build(),
        ] {
            self.activations
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship activation index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(doc! { "launch_id": 1 })
                .options(unique_named("sponsorship_grant_by_launch"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "contributor": 1, "created_at": -1, "_id": -1 })
                .options(named("sponsorship_grants_by_contributor"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "sponsor_subject": 1, "created_at": -1, "_id": -1 })
                .options(named("sponsorship_grants_by_sponsor"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "policy_id": 1, "state": 1, "expires_at": 1, "_id": 1 })
                .options(named("sponsorship_grants_by_policy"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "expires_at": 1, "_id": 1 })
                .options(named("sponsorship_grants_due"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "updated_at": 1, "_id": 1 })
                .options(named("sponsorship_grants_recovery"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "encryption_key_id": 1, "_id": 1 })
                .options(named("sponsorship_grants_by_encryption_key"))
                .build(),
        ] {
            self.grants
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship grant index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(doc! { "request_aliases": 1 })
                .options(unique_named("sponsorship_usage_by_request_alias"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "grant_id": 1, "created_at": -1, "_id": -1 })
                .options(named("sponsorship_usage_by_grant"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "policy_id": 1, "rate_observed_at": -1, "_id": -1 })
                .options(named("sponsorship_usage_rate_window"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "updated_at": 1, "_id": 1 })
                .options(named("sponsorship_usage_ambiguous"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "created_at": 1, "_id": 1 })
                .options(named("sponsorship_usage_kill_switch"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "credential_id": 1, "state": 1, "_id": 1 })
                .options(named("sponsorship_usage_by_credential"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "encryption_key_id": 1, "_id": 1 })
                .options(named("sponsorship_usage_by_encryption_key"))
                .build(),
        ] {
            self.reservations
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship usage index", error))?;
        }
        for index in [
            IndexModel::builder()
                .keys(doc! { "actor_digest": 1, "occurred_at": -1, "_id": -1 })
                .options(named("sponsorship_audit_by_actor"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "policy_id": 1, "occurred_at": -1, "_id": -1 })
                .options(named("sponsorship_audit_by_policy"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "grant_id": 1, "occurred_at": -1, "_id": -1 })
                .options(named("sponsorship_audit_by_grant"))
                .build(),
        ] {
            self.audits
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship audit index", error))?;
        }
        Ok(())
    }

    async fn validate_operational_dependencies(&self) -> Result<(), SponsorshipStoreError> {
        let mut provider_aliases = BTreeSet::new();
        let mut credentials = self.credentials.find(doc! {}).await.map_err(|error| {
            stored_read_failure("inventory credential operational identities", error)
        })?;
        while let Some(credential) = credentials.try_next().await.map_err(|error| {
            stored_read_failure("inventory credential operational identities", error)
        })? {
            validate_credential_provider_aliases(&self.operational, &credential)?;
            read_credential_record(&self.cipher, &self.operational, &credential)?;
            for alias in &credential.provider_credential_id_aliases {
                if !provider_aliases.insert((credential.provider.clone(), alias.clone())) {
                    return Err(SponsorshipStoreError::Malformed(
                        "provider credential identity alias is bound to multiple credentials"
                            .into(),
                    ));
                }
            }
        }
        let mut credential_operations =
            self.credential_operations
                .find(doc! {})
                .await
                .map_err(|error| {
                    stored_read_failure("inventory credential operation identities", error)
                })?;
        while let Some(operation) = credential_operations.try_next().await.map_err(|error| {
            stored_read_failure("inventory credential operation identities", error)
        })? {
            self.operational
                .parse_configured(operation.id.client_request_id_digest.clone())
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
            read_ledger(operation)?;
        }
        let mut registrations =
            self.credential_registrations
                .find(doc! {})
                .await
                .map_err(|error| {
                    stored_read_failure("inventory credential registration aliases", error)
                })?;
        while let Some(registration) = registrations.try_next().await.map_err(|error| {
            stored_read_failure("inventory credential registration aliases", error)
        })? {
            validate_credential_registration_doc(&self.operational, &registration)?;
        }
        let mut policy_creation_aliases = BTreeSet::new();
        let mut policy_creations =
            self.policy_creations.find(doc! {}).await.map_err(|error| {
                stored_read_failure("inventory policy creation identities", error)
            })?;
        while let Some(creation) = policy_creations
            .try_next()
            .await
            .map_err(|error| stored_read_failure("inventory policy creation identities", error))?
        {
            validate_policy_creation_doc(&self.operational, &creation)?;
            for alias in &creation.request_aliases {
                if !policy_creation_aliases.insert(alias.clone()) {
                    return Err(SponsorshipStoreError::Malformed(
                        "policy creation request alias is bound to multiple policies".into(),
                    ));
                }
            }
            let policy_id = SponsorshipPolicyId::parse(creation.policy_id.clone())
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
            if self
                .policies
                .find_one(doc! { "_id": &policy_id.0 })
                .await
                .map_err(|error| stored_read_failure("read policy creation policy", error))?
                .is_none()
            {
                return Err(SponsorshipStoreError::Malformed(
                    "policy creation identity points to no policy".into(),
                ));
            }
        }
        let mut acceptance_aliases = BTreeSet::new();
        let mut acceptances = self.acceptances.find(doc! {}).await.map_err(|error| {
            stored_read_failure("inventory acceptance operational identities", error)
        })?;
        while let Some(acceptance) = acceptances.try_next().await.map_err(|error| {
            stored_read_failure("inventory acceptance operational identities", error)
        })? {
            read_acceptance(&self.operational, acceptance.clone())?;
            for alias in acceptance.idempotency_aliases {
                if !acceptance_aliases.insert(alias) {
                    return Err(SponsorshipStoreError::Malformed(
                        "acceptance idempotency alias is bound to multiple launches".into(),
                    ));
                }
            }
        }
        let mut activations = self.activations.find(doc! {}).await.map_err(|error| {
            stored_read_failure("inventory activation operational identities", error)
        })?;
        while let Some(activation) = activations.try_next().await.map_err(|error| {
            stored_read_failure("inventory activation operational identities", error)
        })? {
            read_activation(&self.cipher, &self.operational, activation)?;
        }
        let mut grants = self.grants.find(doc! {}).await.map_err(|error| {
            stored_read_failure("inventory grant operational identities", error)
        })?;
        while let Some(grant) = grants
            .try_next()
            .await
            .map_err(|error| stored_read_failure("inventory grant operational identities", error))?
        {
            read_grant(&self.cipher, &self.operational, grant)?;
        }
        let mut reservation_aliases = BTreeSet::new();
        let mut reservations = self.reservations.find(doc! {}).await.map_err(|error| {
            stored_read_failure("inventory usage request operational identities", error)
        })?;
        while let Some(reservation) = reservations.try_next().await.map_err(|error| {
            stored_read_failure("inventory usage request operational identities", error)
        })? {
            read_reservation(&self.operational, reservation.clone())?;
            for alias in reservation.request_aliases {
                if !reservation_aliases.insert(alias) {
                    return Err(SponsorshipStoreError::Malformed(
                        "usage request alias is bound to multiple reservations".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    async fn transaction<R, C, F>(
        &self,
        context: C,
        callback: F,
    ) -> Result<R, SponsorshipStoreError>
    where
        F: for<'a> FnMut(
            &'a mut ClientSession,
            &'a mut C,
        ) -> BoxFuture<'a, mongodb::error::Result<R>>,
    {
        let mut session = self
            .client
            .start_session()
            .await
            .map_err(|error| unavailable("start sponsorship persistence transaction", error))?;
        let result = session
            .start_transaction()
            .read_concern(ReadConcern::snapshot())
            .write_concern(WriteConcern::majority())
            .and_run(context, callback)
            .await;
        map_transaction_result(result)
    }

    async fn database_now(&self, session: &mut ClientSession) -> mongodb::error::Result<DateTime> {
        #[derive(Debug, Deserialize)]
        struct ClockDoc {
            now: DateTime,
        }
        let mut cursor = self
            .gate
            .aggregate([
                doc! { "$match": { "_id": GATE_ID } },
                doc! { "$project": { "_id": 0, "now": "$$NOW" } },
            ])
            .session(&mut *session)
            .await?;
        let document = cursor
            .next(&mut *session)
            .await
            .transpose()?
            .ok_or_else(|| {
                Error::custom(SponsorshipStoreError::Malformed(
                    "the sponsorship gate clock authority is missing".into(),
                ))
            })?;
        mongodb::bson::deserialize_from_document::<ClockDoc>(document)
            .map(|clock| clock.now)
            .map_err(|_| {
                Error::custom(SponsorshipStoreError::Malformed(
                    "the sponsorship database clock is malformed".into(),
                ))
            })
    }

    async fn ensure_sponsor_counter(&self, sponsor_key: &str) -> Result<(), SponsorshipStoreError> {
        if !is_digest(sponsor_key) {
            return Err(SponsorshipStoreError::Malformed(
                "sponsorship sponsor authority key is invalid".into(),
            ));
        }
        let candidate = SponsorCounterDoc {
            sponsor_key: sponsor_key.to_string(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            policy_revision: 0,
            launch_revision: 0,
        };
        match self.sponsor_counters.insert_one(&candidate).await {
            Ok(_) => Ok(()),
            Err(error) if is_duplicate_key(&error) => {
                let stored = self
                    .sponsor_counters
                    .find_one(doc! { "_id": sponsor_key })
                    .await
                    .map_err(|error| {
                        stored_read_failure("read sponsorship sponsor counter", error)
                    })?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Unavailable(
                            "sponsorship sponsor counter disappeared during initialization".into(),
                        )
                    })?;
                validate_sponsor_counter(&stored, sponsor_key)
            }
            Err(error) => Err(unavailable("initialize sponsorship sponsor counter", error)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum SponsorAdmissionFence {
    Policy,
    Launch,
}

fn validate_sponsor_counter(
    counter: &SponsorCounterDoc,
    sponsor_key: &str,
) -> Result<(), SponsorshipStoreError> {
    validate_document_version(
        counter.document_schema_version,
        "sponsorship sponsor admission counter",
    )?;
    if counter.sponsor_key != sponsor_key
        || !is_digest(&counter.sponsor_key)
        || counter.policy_revision < 0
        || counter.launch_revision < 0
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship sponsor admission counter is invalid".into(),
        ));
    }
    Ok(())
}

fn operation_kind_code(kind: ProviderOperationKind) -> &'static str {
    match kind {
        ProviderOperationKind::RotateCredential => "rotate_credential",
        ProviderOperationKind::RevokeCredential => "revoke_credential",
        ProviderOperationKind::RevokeGrant => "revoke_grant",
        ProviderOperationKind::CloseGrant => "close_grant",
        ProviderOperationKind::RevokeUncommittedGrant => "revoke_uncommitted_grant",
    }
}

fn parse_operation_kind(value: &str) -> Result<ProviderOperationKind, SponsorshipStoreError> {
    match value {
        "rotate_credential" => Ok(ProviderOperationKind::RotateCredential),
        "revoke_credential" => Ok(ProviderOperationKind::RevokeCredential),
        "revoke_grant" => Ok(ProviderOperationKind::RevokeGrant),
        "close_grant" => Ok(ProviderOperationKind::CloseGrant),
        "revoke_uncommitted_grant" => Ok(ProviderOperationKind::RevokeUncommittedGrant),
        _ => Err(malformed_code("provider operation kind", value)),
    }
}

fn operation_phase_code(phase: ProviderOperationPhase) -> &'static str {
    match phase {
        ProviderOperationPhase::Prepared => "prepared",
        ProviderOperationPhase::Attempting => "attempting",
        ProviderOperationPhase::Ambiguous => "ambiguous",
        ProviderOperationPhase::Rejected => "rejected",
    }
}

fn parse_operation_phase(value: &str) -> Result<ProviderOperationPhase, SponsorshipStoreError> {
    match value {
        "prepared" => Ok(ProviderOperationPhase::Prepared),
        "attempting" => Ok(ProviderOperationPhase::Attempting),
        "ambiguous" => Ok(ProviderOperationPhase::Ambiguous),
        "rejected" => Ok(ProviderOperationPhase::Rejected),
        _ => Err(malformed_code("provider operation phase", value)),
    }
}

fn completed_outcome_code(outcome: CompletedProviderOperationOutcome) -> &'static str {
    match outcome {
        CompletedProviderOperationOutcome::Confirmed => "confirmed",
        CompletedProviderOperationOutcome::Rejected => "rejected",
    }
}

fn parse_completed_outcome(
    value: &str,
) -> Result<CompletedProviderOperationOutcome, SponsorshipStoreError> {
    match value {
        "confirmed" => Ok(CompletedProviderOperationOutcome::Confirmed),
        "rejected" => Ok(CompletedProviderOperationOutcome::Rejected),
        _ => Err(malformed_code(
            "completed provider operation outcome",
            value,
        )),
    }
}

fn operation_doc(operation: &PendingProviderOperation) -> ProviderOperationDoc {
    ProviderOperationDoc {
        kind: operation_kind_code(operation.kind).into(),
        phase: operation_phase_code(operation.phase).into(),
        provider_request_id: operation.provider_request_id.clone(),
        client_request_id_digest: operation.client_request_id_digest.clone(),
        payload_digest: operation.payload_digest.clone(),
        requested_at: operation.requested_at.clone(),
        attempted_at: operation.attempted_at.clone(),
    }
}

fn read_operation(
    document: ProviderOperationDoc,
) -> Result<PendingProviderOperation, SponsorshipStoreError> {
    let operation = PendingProviderOperation {
        kind: parse_operation_kind(&document.kind)?,
        phase: parse_operation_phase(&document.phase)?,
        provider_request_id: document.provider_request_id,
        client_request_id_digest: document.client_request_id_digest,
        payload_digest: document.payload_digest,
        requested_at: document.requested_at,
        attempted_at: document.attempted_at,
    };
    operation
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(operation)
}

fn completed_operation_doc(
    operation: &CompletedProviderOperation,
) -> CompletedProviderOperationDoc {
    CompletedProviderOperationDoc {
        kind: operation_kind_code(operation.kind).into(),
        provider_request_id: operation.provider_request_id.clone(),
        client_request_id_digest: operation.client_request_id_digest.clone(),
        payload_digest: operation.payload_digest.clone(),
        outcome: completed_outcome_code(operation.outcome).into(),
        completed_at: operation.completed_at.clone(),
    }
}

fn read_completed_operation(
    document: CompletedProviderOperationDoc,
) -> Result<CompletedProviderOperation, SponsorshipStoreError> {
    let operation = CompletedProviderOperation {
        kind: parse_operation_kind(&document.kind)?,
        provider_request_id: document.provider_request_id,
        client_request_id_digest: document.client_request_id_digest,
        payload_digest: document.payload_digest,
        outcome: parse_completed_outcome(&document.outcome)?,
        completed_at: document.completed_at,
    };
    operation
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(operation)
}

fn credential_state_code(state: SponsoredCredentialState) -> &'static str {
    match state {
        SponsoredCredentialState::Active => "active",
        SponsoredCredentialState::RotationPending => "rotation_pending",
        SponsoredCredentialState::RevocationPending => "revocation_pending",
        SponsoredCredentialState::Revoked => "revoked",
    }
}

fn parse_credential_state(value: &str) -> Result<SponsoredCredentialState, SponsorshipStoreError> {
    match value {
        "active" => Ok(SponsoredCredentialState::Active),
        "rotation_pending" => Ok(SponsoredCredentialState::RotationPending),
        "revocation_pending" => Ok(SponsoredCredentialState::RevocationPending),
        "revoked" => Ok(SponsoredCredentialState::Revoked),
        _ => Err(malformed_code("sponsored credential state", value)),
    }
}

fn material_state_code(state: SponsoredCredentialMaterialState) -> &'static str {
    match state {
        SponsoredCredentialMaterialState::Retained => "retained",
        SponsoredCredentialMaterialState::Retired => "retired",
    }
}

fn parse_material_state(
    value: &str,
) -> Result<SponsoredCredentialMaterialState, SponsorshipStoreError> {
    match value {
        "retained" => Ok(SponsoredCredentialMaterialState::Retained),
        "retired" => Ok(SponsoredCredentialMaterialState::Retired),
        _ => Err(malformed_code("sponsored credential material state", value)),
    }
}

struct DecodedCredential {
    record: SponsoredCredentialRecord,
    provider_credential_id_aliases: Vec<String>,
    current_secret: Option<ProviderCredentialSecret>,
    pending_secret: Option<ProviderCredentialSecret>,
    authority_read_revision: i64,
}

fn credential_doc(
    cipher: &SponsorshipCipher,
    record: &SponsoredCredentialRecord,
    provider_credential_id_aliases: &[String],
    current_secret: Option<&str>,
    pending_secret: Option<&str>,
    authority_read_revision: i64,
) -> Result<CredentialDoc, SponsorshipStoreError> {
    record
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let record_id = record.id.0.as_str();
    let authority_context = credential_cipher_context(
        record_id,
        &record.owner,
        &record.authority.provider,
        &record.authority.provider_policy_key,
        &record.authority.provider_credential_id_digest,
        &record.authority.ownership_verified_at,
        &record.created_at,
        record.authority.state_version,
        record.authority.supports_expiry,
        record.authority.supports_revocation,
    )?;
    let current_secret = current_secret
        .map(|secret| {
            cipher.seal(
                "credential",
                record_id,
                "current_secret",
                &authority_context,
                secret,
            )
        })
        .transpose()?;
    let pending_secret = pending_secret
        .map(|secret| {
            cipher.seal(
                "credential",
                record_id,
                "pending_secret",
                &authority_context,
                secret,
            )
        })
        .transpose()?;
    if authority_read_revision < 0 {
        return Err(SponsorshipStoreError::Malformed(
            "credential authority read revision is negative".into(),
        ));
    }
    let retained = record.material_state == SponsoredCredentialMaterialState::Retained;
    let pending_material_required = record.state == SponsoredCredentialState::RotationPending
        || record.superseded_operation.is_some();
    if retained != current_secret.is_some()
        || pending_material_required != pending_secret.is_some()
        || (record.material_state == SponsoredCredentialMaterialState::Retired
            && pending_secret.is_some())
    {
        return Err(SponsorshipStoreError::Malformed(
            "credential ciphertext does not match the lifecycle material state".into(),
        ));
    }
    Ok(CredentialDoc {
        id: record.id.0.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        owner: record.owner.clone(),
        owner_subject: record.owner.controller().0.clone(),
        provider: record.authority.provider.clone(),
        provider_policy_key: record.authority.provider_policy_key.clone(),
        provider_policy_handle: cipher.seal(
            "credential",
            record_id,
            "provider_policy_handle",
            &authority_context,
            record
                .authority
                .provider_policy_handle
                .expose_for_provider_or_encryption(),
        )?,
        provider_credential_id_digest: record.authority.provider_credential_id_digest.clone(),
        provider_credential_id_aliases: provider_credential_id_aliases.to_vec(),
        provider_state_version: signed(
            record.authority.state_version,
            "credential provider state version",
        )?,
        ownership_verified_at: record.authority.ownership_verified_at.clone(),
        supports_expiry: record.authority.supports_expiry,
        supports_revocation: record.authority.supports_revocation,
        state: credential_state_code(record.state).into(),
        material_state: material_state_code(record.material_state).into(),
        generation: signed(record.generation, "credential generation")?,
        created_at: record.created_at.clone(),
        updated_at: record.updated_at.clone(),
        revoked_at: record.revoked_at.clone(),
        pending_operation: record.pending_operation.as_ref().map(operation_doc),
        superseded_operation: record.superseded_operation.as_ref().map(operation_doc),
        last_completed_operation: record
            .last_completed_operation
            .as_ref()
            .map(completed_operation_doc),
        current_secret,
        pending_secret,
        encryption_key_id: cipher.current_key_id().into(),
        authority_read_revision,
    })
}

fn read_credential_record(
    cipher: &SponsorshipCipher,
    operational: &SponsorshipOperationalPseudonymizer,
    document: &CredentialDoc,
) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship credential")?;
    validate_credential_provider_aliases(operational, document)?;
    validate_credential_operation_dependencies(operational, document)?;
    if document.authority_read_revision < 0 {
        return Err(SponsorshipStoreError::Malformed(
            "credential authority read revision is negative".into(),
        ));
    }
    let id = SponsoredCredentialId::parse(document.id.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let authority_context = credential_cipher_context(
        &document.id,
        &document.owner,
        &document.provider,
        &document.provider_policy_key,
        &document.provider_credential_id_digest,
        &document.ownership_verified_at,
        &document.created_at,
        unsigned(
            document.provider_state_version,
            "credential provider state version",
        )?,
        document.supports_expiry,
        document.supports_revocation,
    )?;
    for envelope in [
        Some(&document.provider_policy_handle),
        document.current_secret.as_ref(),
        document.pending_secret.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if envelope.key_id() != document.encryption_key_id {
            return Err(SponsorshipStoreError::Malformed(
                "credential encryption-key index does not match its envelopes".into(),
            ));
        }
    }
    let handle = cipher.open(
        "credential",
        &document.id,
        "provider_policy_handle",
        &authority_context,
        &document.provider_policy_handle,
    )?;
    let record = SponsoredCredentialRecord {
        schema_version: layer2_core::SPONSORSHIP_SCHEMA_VERSION,
        id,
        owner: document.owner.clone(),
        authority: ProviderCredentialAuthority {
            provider: document.provider.clone(),
            provider_policy_key: document.provider_policy_key.clone(),
            provider_policy_handle: SensitiveProviderValue::new(handle.plaintext.to_string())
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
            provider_credential_id_digest: document.provider_credential_id_digest.clone(),
            state_version: unsigned(
                document.provider_state_version,
                "credential provider state version",
            )?,
            ownership_verified_at: document.ownership_verified_at.clone(),
            supports_expiry: document.supports_expiry,
            supports_revocation: document.supports_revocation,
        },
        state: parse_credential_state(&document.state)?,
        material_state: parse_material_state(&document.material_state)?,
        generation: unsigned(document.generation, "credential generation")?,
        created_at: document.created_at.clone(),
        updated_at: document.updated_at.clone(),
        revoked_at: document.revoked_at.clone(),
        pending_operation: document
            .pending_operation
            .clone()
            .map(read_operation)
            .transpose()?,
        superseded_operation: document
            .superseded_operation
            .clone()
            .map(read_operation)
            .transpose()?,
        last_completed_operation: document
            .last_completed_operation
            .clone()
            .map(read_completed_operation)
            .transpose()?,
    };
    record
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let retained = record.material_state == SponsoredCredentialMaterialState::Retained;
    let pending_material_required = record.state == SponsoredCredentialState::RotationPending
        || record.superseded_operation.is_some();
    if document.owner_subject != record.owner.controller().0
        || retained != document.current_secret.is_some()
        || pending_material_required != document.pending_secret.is_some()
        || (record.material_state == SponsoredCredentialMaterialState::Retired
            && document.pending_secret.is_some())
    {
        return Err(SponsorshipStoreError::Malformed(
            "stored credential material does not match its lifecycle state".into(),
        ));
    }
    Ok(record)
}

fn read_credential(
    cipher: &SponsorshipCipher,
    operational: &SponsorshipOperationalPseudonymizer,
    document: CredentialDoc,
) -> Result<DecodedCredential, SponsorshipStoreError> {
    let record = read_credential_record(cipher, operational, &document)?;
    let authority_context = credential_cipher_context(
        &document.id,
        &document.owner,
        &document.provider,
        &document.provider_policy_key,
        &document.provider_credential_id_digest,
        &document.ownership_verified_at,
        &document.created_at,
        record.authority.state_version,
        document.supports_expiry,
        document.supports_revocation,
    )?;
    let current_secret = document
        .current_secret
        .as_ref()
        .map(|envelope| {
            cipher
                .open(
                    "credential",
                    &document.id,
                    "current_secret",
                    &authority_context,
                    envelope,
                )
                .and_then(|opened| {
                    ProviderCredentialSecret::new(opened.plaintext.to_string())
                        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))
                })
        })
        .transpose()?;
    let pending_secret = document
        .pending_secret
        .as_ref()
        .map(|envelope| {
            cipher
                .open(
                    "credential",
                    &document.id,
                    "pending_secret",
                    &authority_context,
                    envelope,
                )
                .and_then(|opened| {
                    ProviderCredentialSecret::new(opened.plaintext.to_string())
                        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))
                })
        })
        .transpose()?;
    Ok(DecodedCredential {
        record,
        provider_credential_id_aliases: document.provider_credential_id_aliases,
        current_secret,
        pending_secret,
        authority_read_revision: document.authority_read_revision,
    })
}

fn policy_state_code(policy: &SponsorshipPolicy) -> &'static str {
    match policy.state {
        layer2_core::SponsorshipPolicyState::Active => "active",
        layer2_core::SponsorshipPolicyState::Suspended => "suspended",
        layer2_core::SponsorshipPolicyState::Revoked => "revoked",
    }
}

fn policy_doc(
    policy: &SponsorshipPolicy,
    usage: &SponsorshipPolicyUsage,
) -> Result<PolicyDoc, SponsorshipStoreError> {
    policy
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    usage
        .exposure()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(PolicyDoc {
        id: policy.id.0.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        owner_subject: policy.owner.controller().0.clone(),
        sponsor_key: policy
            .owner
            .sponsorship_authority_key()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        credential_id: policy.credential_id.0.clone(),
        state: policy_state_code(policy).into(),
        expires_at: policy.expires_at.clone(),
        expires_at_database: timestamp_date_time(&policy.expires_at, "sponsorship policy expiry")?,
        created_at: policy.created_at.clone(),
        updated_at: policy.updated_at.clone(),
        policy: policy.clone(),
        usage: PolicyUsageDoc {
            consumed: usage.consumed.clone(),
            reserved: usage.reserved.clone(),
            active_launches: i64::from(usage.active_launches),
        },
        authority_read_revision: 0,
    })
}

fn read_policy(
    document: PolicyDoc,
) -> Result<(SponsorshipPolicy, SponsorshipPolicyUsage), SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship policy")?;
    if document.authority_read_revision < 0 {
        return Err(SponsorshipStoreError::Malformed(
            "policy authority read revision is negative".into(),
        ));
    }
    document
        .policy
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let sponsor_key = document
        .policy
        .owner
        .sponsorship_authority_key()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if document.id != document.policy.id.0
        || document.owner_subject != document.policy.owner.controller().0
        || document.sponsor_key != sponsor_key
        || document.credential_id != document.policy.credential_id.0
        || document.state != policy_state_code(&document.policy)
        || document.expires_at != document.policy.expires_at
        || document.expires_at_database
            != timestamp_date_time(&document.policy.expires_at, "sponsorship policy expiry")?
        || document.created_at != document.policy.created_at
        || document.updated_at != document.policy.updated_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship policy indexes do not match the canonical record".into(),
        ));
    }
    let usage = SponsorshipPolicyUsage {
        consumed: document.usage.consumed,
        reserved: document.usage.reserved,
        active_launches: u32::try_from(document.usage.active_launches).map_err(|_| {
            SponsorshipStoreError::Malformed(
                "stored sponsorship active-Launch count is invalid".into(),
            )
        })?,
    };
    usage
        .exposure()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if usage.active_launches > document.policy.max_concurrent_launches {
        return Err(SponsorshipStoreError::Malformed(
            "stored sponsorship active-Launch count exceeds its policy".into(),
        ));
    }
    Ok((document.policy, usage))
}

fn acceptance_doc(
    operational: &SponsorshipOperationalPseudonymizer,
    acceptance: &SponsoredGrantAcceptance,
    idempotency_candidates: &SponsorshipOperationalPseudonymSet,
) -> Result<AcceptanceDoc, SponsorshipStoreError> {
    acceptance
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let idempotency_aliases = idempotency_candidates.encoded_candidates();
    let aliases =
        validate_operational_aliases(operational, &idempotency_aliases, "acceptance idempotency")?;
    if acceptance.idempotency_key_hash != idempotency_candidates.current().as_str()
        || !aliases.contains(&acceptance.idempotency_key_hash)
    {
        return Err(SponsorshipStoreError::Malformed(
            "new acceptance canonical idempotency identity is not the current candidate".into(),
        ));
    }
    Ok(AcceptanceDoc {
        launch_id: acceptance.launch_id.0.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        idempotency_aliases,
        contributor: acceptance.contributor.0.clone(),
        policy_id: acceptance.policy_id.0.clone(),
        accepted_at: acceptance.accepted_at.clone(),
        acceptance: acceptance.clone(),
        authority_read_revision: 0,
    })
}

fn read_acceptance(
    operational: &SponsorshipOperationalPseudonymizer,
    document: AcceptanceDoc,
) -> Result<SponsoredGrantAcceptance, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship acceptance")?;
    if document.authority_read_revision < 0 {
        return Err(SponsorshipStoreError::Malformed(
            "acceptance authority read revision is negative".into(),
        ));
    }
    document
        .acceptance
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let aliases = validate_operational_aliases(
        operational,
        &document.idempotency_aliases,
        "acceptance idempotency",
    )?;
    let canonical = operational
        .parse_configured(document.acceptance.idempotency_key_hash.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if !aliases.contains(canonical.as_str()) {
        return Err(SponsorshipStoreError::Malformed(
            "acceptance idempotency aliases omit the canonical identity".into(),
        ));
    }
    if document.launch_id != document.acceptance.launch_id.0
        || document.contributor != document.acceptance.contributor.0
        || document.policy_id != document.acceptance.policy_id.0
        || document.accepted_at != document.acceptance.accepted_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship acceptance indexes do not match the canonical record".into(),
        ));
    }
    Ok(document.acceptance)
}

fn gate_fence_doc(fence: &SponsorshipGateFence) -> Result<GateFenceDoc, SponsorshipStoreError> {
    validate_gate(fence)?;
    Ok(GateFenceDoc {
        configuration_digest: fence.configuration_digest.clone(),
        epoch: signed(fence.epoch, "activation gate epoch")?,
        kill_switch_engaged: fence.kill_switch_engaged,
        trusted_provider_policy_key: fence.trusted_provider_policy_key.clone(),
        runtime_limits: fence.runtime_limits.clone(),
    })
}

fn read_gate_fence(document: GateFenceDoc) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
    let fence = SponsorshipGateFence {
        configuration_digest: document.configuration_digest,
        epoch: unsigned(document.epoch, "activation gate epoch")?,
        kill_switch_engaged: document.kill_switch_engaged,
        trusted_provider_policy_key: document.trusted_provider_policy_key,
        runtime_limits: document.runtime_limits,
    };
    validate_gate(&fence)?;
    Ok(fence)
}

fn activation_state_code(state: GrantActivationState) -> &'static str {
    match state {
        GrantActivationState::AwaitingBilling => "awaiting_billing",
        GrantActivationState::BillingAttempting => "billing_attempting",
        GrantActivationState::BillingAmbiguous => "billing_ambiguous",
        GrantActivationState::AwaitingProvider => "awaiting_provider",
        GrantActivationState::ProviderAttempting => "provider_attempting",
        GrantActivationState::ProviderAmbiguous => "provider_ambiguous",
        GrantActivationState::ProviderConfirmed => "provider_confirmed",
        GrantActivationState::CleanupPending => "cleanup_pending",
        GrantActivationState::Cleaned => "cleaned",
        GrantActivationState::Denied => "denied",
        GrantActivationState::Committed => "committed",
    }
}

fn parse_activation_state(value: &str) -> Result<GrantActivationState, SponsorshipStoreError> {
    match value {
        "awaiting_billing" => Ok(GrantActivationState::AwaitingBilling),
        "billing_attempting" => Ok(GrantActivationState::BillingAttempting),
        "billing_ambiguous" => Ok(GrantActivationState::BillingAmbiguous),
        "awaiting_provider" => Ok(GrantActivationState::AwaitingProvider),
        "provider_attempting" => Ok(GrantActivationState::ProviderAttempting),
        "provider_ambiguous" => Ok(GrantActivationState::ProviderAmbiguous),
        "provider_confirmed" => Ok(GrantActivationState::ProviderConfirmed),
        "cleanup_pending" => Ok(GrantActivationState::CleanupPending),
        "cleaned" => Ok(GrantActivationState::Cleaned),
        "denied" => Ok(GrantActivationState::Denied),
        "committed" => Ok(GrantActivationState::Committed),
        _ => Err(malformed_code("grant activation state", value)),
    }
}

fn activation_denial_code(denial: GrantActivationDenial) -> &'static str {
    match denial {
        GrantActivationDenial::BillingRejected => "billing_rejected",
        GrantActivationDenial::ProviderRejected => "provider_rejected",
        GrantActivationDenial::AuthorityInvalidated => "authority_invalidated",
    }
}

fn parse_activation_denial(value: &str) -> Result<GrantActivationDenial, SponsorshipStoreError> {
    match value {
        "billing_rejected" => Ok(GrantActivationDenial::BillingRejected),
        "provider_rejected" => Ok(GrantActivationDenial::ProviderRejected),
        "authority_invalidated" => Ok(GrantActivationDenial::AuthorityInvalidated),
        _ => Err(malformed_code("grant activation denial", value)),
    }
}

fn provider_receipt_doc(
    cipher: &SponsorshipCipher,
    record_kind: &str,
    record_id: &str,
    authority_context: &str,
    receipt: &ProviderGrantReceipt,
) -> Result<ProviderReceiptDoc, SponsorshipStoreError> {
    Ok(ProviderReceiptDoc {
        provider_grant_id: cipher.seal(
            record_kind,
            record_id,
            "provider_grant_id",
            authority_context,
            receipt
                .provider_grant_id
                .expose_for_provider_or_encryption(),
        )?,
        provider_state_version: signed(
            receipt.provider_state_version,
            "provider grant state version",
        )?,
        expires_at: receipt.expires_at.clone(),
        revocation_handle: cipher.seal(
            record_kind,
            record_id,
            "revocation_handle",
            authority_context,
            receipt
                .revocation_handle
                .expose_for_provider_or_encryption(),
        )?,
        issued_at: receipt.issued_at.clone(),
    })
}

fn read_provider_receipt(
    cipher: &SponsorshipCipher,
    record_kind: &str,
    record_id: &str,
    authority_context: &str,
    document: ProviderReceiptDoc,
    encryption_key_id: &str,
) -> Result<ProviderGrantReceipt, SponsorshipStoreError> {
    if document.provider_grant_id.key_id() != encryption_key_id
        || document.revocation_handle.key_id() != encryption_key_id
    {
        return Err(SponsorshipStoreError::Malformed(
            "provider receipt encryption-key index does not match its envelopes".into(),
        ));
    }
    let provider_grant_id = cipher.open(
        record_kind,
        record_id,
        "provider_grant_id",
        authority_context,
        &document.provider_grant_id,
    )?;
    let revocation_handle = cipher.open(
        record_kind,
        record_id,
        "revocation_handle",
        authority_context,
        &document.revocation_handle,
    )?;
    Ok(ProviderGrantReceipt {
        provider_grant_id: SensitiveProviderValue::new(provider_grant_id.plaintext.to_string())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        provider_state_version: unsigned(
            document.provider_state_version,
            "provider grant state version",
        )?,
        expires_at: document.expires_at,
        revocation_handle: SensitiveProviderValue::new(revocation_handle.plaintext.to_string())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        issued_at: document.issued_at,
    })
}

fn activation_doc(
    cipher: &SponsorshipCipher,
    intent: &GrantActivationIntent,
    lease_expiry_database: Option<DateTime>,
) -> Result<ActivationDoc, SponsorshipStoreError> {
    intent
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let authority_context = activation_cipher_context(
        &intent.launch_id.0,
        &intent.session_id.0,
        &intent.policy_id.0,
        intent.policy_generation,
        intent.credential_generation,
        &intent.launch_authority_digest,
        &intent.acceptance_digest,
        &intent.gate_fence,
        &intent.allocation,
        &intent.request_fingerprint,
        &intent.billing_request_id,
        &intent.provider_request_id,
        &intent.created_at,
    )?;
    let lease = match (&intent.lease, lease_expiry_database) {
        (Some(lease), Some(expires_at_database)) => {
            validate_activation_lease_database_binding(lease, expires_at_database)?;
            Some(ActivationLeaseDoc {
                holder: lease.holder.clone(),
                acquired_at: lease.acquired_at.clone(),
                expires_at: lease.expires_at.clone(),
                expires_at_database,
            })
        }
        (None, None) => None,
        _ => {
            return Err(SponsorshipStoreError::Malformed(
                "activation lease lacks its database-time expiry".into(),
            ))
        }
    };
    Ok(ActivationDoc {
        launch_id: intent.launch_id.0.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        session_id: intent.session_id.0.clone(),
        policy_id: intent.policy_id.0.clone(),
        policy_generation: signed(intent.policy_generation, "activation policy generation")?,
        credential_generation: signed(
            intent.credential_generation,
            "activation credential generation",
        )?,
        launch_authority_digest: intent.launch_authority_digest.clone(),
        acceptance_digest: intent.acceptance_digest.clone(),
        gate_fence: gate_fence_doc(&intent.gate_fence)?,
        allocation: intent.allocation.clone(),
        request_fingerprint: intent.request_fingerprint.clone(),
        billing_request_id: intent.billing_request_id.clone(),
        provider_request_id: intent.provider_request_id.clone(),
        state: activation_state_code(intent.state).into(),
        denial: intent
            .denial
            .map(activation_denial_code)
            .map(str::to_string),
        capacity_held: intent.capacity_held,
        revision: signed(intent.revision, "activation revision")?,
        lease,
        provider_receipt: intent
            .provider_receipt
            .as_ref()
            .map(|receipt| {
                provider_receipt_doc(
                    cipher,
                    "activation",
                    &intent.launch_id.0,
                    &authority_context,
                    receipt,
                )
            })
            .transpose()?,
        cleanup_operation: intent.cleanup_operation.as_ref().map(operation_doc),
        created_at: intent.created_at.clone(),
        updated_at: intent.updated_at.clone(),
        encryption_key_id: cipher.current_key_id().into(),
    })
}

fn read_activation(
    cipher: &SponsorshipCipher,
    operational: &SponsorshipOperationalPseudonymizer,
    document: ActivationDoc,
) -> Result<(GrantActivationIntent, Option<DateTime>), SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "grant activation")?;
    let revision = unsigned(document.revision, "activation revision")?;
    let policy_generation = unsigned(document.policy_generation, "activation policy generation")?;
    let credential_generation = unsigned(
        document.credential_generation,
        "activation credential generation",
    )?;
    let gate_fence = read_gate_fence(document.gate_fence.clone())?;
    let authority_context = activation_cipher_context(
        &document.launch_id,
        &document.session_id,
        &document.policy_id,
        policy_generation,
        credential_generation,
        &document.launch_authority_digest,
        &document.acceptance_digest,
        &gate_fence,
        &document.allocation,
        &document.request_fingerprint,
        &document.billing_request_id,
        &document.provider_request_id,
        &document.created_at,
    )?;
    let lease_expiry_database = document
        .lease
        .as_ref()
        .map(|lease| lease.expires_at_database);
    let intent = GrantActivationIntent {
        launch_id: LaunchId::parse(document.launch_id.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        session_id: SessionId(document.session_id),
        policy_id: SponsorshipPolicyId::parse(document.policy_id)
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        policy_generation,
        credential_generation,
        launch_authority_digest: document.launch_authority_digest,
        acceptance_digest: document.acceptance_digest,
        gate_fence,
        allocation: document.allocation,
        request_fingerprint: document.request_fingerprint,
        billing_request_id: document.billing_request_id,
        provider_request_id: document.provider_request_id,
        state: parse_activation_state(&document.state)?,
        denial: document
            .denial
            .as_deref()
            .map(parse_activation_denial)
            .transpose()?,
        capacity_held: document.capacity_held,
        revision,
        lease: document.lease.map(|lease| GrantActivationLease {
            holder: lease.holder,
            acquired_at: lease.acquired_at,
            expires_at: lease.expires_at,
        }),
        provider_receipt: document
            .provider_receipt
            .map(|receipt| {
                read_provider_receipt(
                    cipher,
                    "activation",
                    &document.launch_id,
                    &authority_context,
                    receipt,
                    &document.encryption_key_id,
                )
            })
            .transpose()?,
        cleanup_operation: document.cleanup_operation.map(read_operation).transpose()?,
        created_at: document.created_at,
        updated_at: document.updated_at,
    };
    intent
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    operational
        .parse_configured(intent.billing_request_id.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    operational
        .parse_configured(intent.provider_request_id.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if let Some(lease) = &intent.lease {
        operational
            .parse_configured(lease.holder.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    }
    if let Some(operation) = &intent.cleanup_operation {
        operational
            .parse_configured(operation.client_request_id_digest.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    }
    if let (Some(lease), Some(expires_at_database)) = (&intent.lease, lease_expiry_database) {
        validate_activation_lease_database_binding(lease, expires_at_database)?;
    }
    Ok((intent, lease_expiry_database))
}

fn grant_state_code(state: SponsoredGrantState) -> &'static str {
    match state {
        SponsoredGrantState::Active => "active",
        SponsoredGrantState::Exhausted => "exhausted",
        SponsoredGrantState::Expired => "expired",
        SponsoredGrantState::RevocationPending => "revocation_pending",
        SponsoredGrantState::Revoked => "revoked",
        SponsoredGrantState::Closed => "closed",
        SponsoredGrantState::Suspended => "suspended",
    }
}

fn parse_grant_state(value: &str) -> Result<SponsoredGrantState, SponsorshipStoreError> {
    match value {
        "active" => Ok(SponsoredGrantState::Active),
        "exhausted" => Ok(SponsoredGrantState::Exhausted),
        "expired" => Ok(SponsoredGrantState::Expired),
        "revocation_pending" => Ok(SponsoredGrantState::RevocationPending),
        "revoked" => Ok(SponsoredGrantState::Revoked),
        "closed" => Ok(SponsoredGrantState::Closed),
        "suspended" => Ok(SponsoredGrantState::Suspended),
        _ => Err(malformed_code("sponsored grant state", value)),
    }
}

fn grant_doc(
    cipher: &SponsorshipCipher,
    operational: &SponsorshipOperationalPseudonymizer,
    grant: &SponsoredGrant,
    receipt: &ProviderGrantReceipt,
    sponsor_subject: &str,
) -> Result<GrantDoc, SponsorshipStoreError> {
    grant
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    require_receipt_matches_grant(operational, receipt, grant)?;
    let authority_context = grant_cipher_context(grant)?;
    let encrypted_receipt =
        provider_receipt_doc(cipher, "grant", &grant.id.0, &authority_context, receipt)?;
    grant_doc_with_encrypted_receipt(
        grant,
        sponsor_subject,
        SponsoredUsageSummary::default(),
        encrypted_receipt,
        cipher.current_key_id().to_string(),
    )
}

fn grant_doc_with_encrypted_receipt(
    grant: &SponsoredGrant,
    sponsor_subject: &str,
    unresolved_usage: SponsoredUsageSummary,
    provider_receipt: ProviderReceiptDoc,
    encryption_key_id: String,
) -> Result<GrantDoc, SponsorshipStoreError> {
    grant
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if sponsor_subject.is_empty()
        || sponsor_subject.len() > 255
        || sponsor_subject != grant.sponsor_subject.0
    {
        return Err(SponsorshipStoreError::Malformed(
            "grant sponsor subject index does not match its canonical authority".into(),
        ));
    }
    let exposure = summary_add(&grant.usage, &unresolved_usage)?;
    if !exposure.fits_within(&grant.allocation) {
        return Err(SponsorshipStoreError::Malformed(
            "grant usage plus unresolved holds exceeds its allocation".into(),
        ));
    }
    if provider_receipt.provider_grant_id.key_id() != encryption_key_id
        || provider_receipt.revocation_handle.key_id() != encryption_key_id
        || unsigned(
            provider_receipt.provider_state_version,
            "provider grant state version",
        )? != grant.authority.provider_state_version
        || provider_receipt.expires_at != grant.authority.expires_at
        || provider_receipt.issued_at != grant.authority.issued_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "encrypted provider receipt metadata does not match the grant authority".into(),
        ));
    }
    Ok(GrantDoc {
        id: grant.id.0.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        schema_version: i64::from(grant.schema_version),
        tenant_id: grant.tenant_id.clone(),
        policy_id: grant.policy_id.0.clone(),
        policy_generation: signed(grant.policy_generation, "grant policy generation")?,
        credential_generation: signed(grant.credential_generation, "grant credential generation")?,
        trigger_revision_id: grant.trigger_revision_id.0.clone(),
        trigger_revision_digest: grant.trigger_revision_digest.clone(),
        launch_id: grant.launch_id.0.clone(),
        session_id: grant.session_id.0.clone(),
        contributor: grant.contributor.0.clone(),
        sponsor_subject: sponsor_subject.to_string(),
        terms_digest: grant.terms_digest.clone(),
        acceptance: grant.acceptance.clone(),
        authority: grant.authority.clone(),
        capability: grant.capability.clone(),
        state: grant_state_code(grant.state).into(),
        allocation: grant.allocation.clone(),
        usage: grant.usage.clone(),
        unresolved_usage,
        generation: signed(grant.generation, "grant generation")?,
        created_at: grant.created_at.clone(),
        updated_at: grant.updated_at.clone(),
        expires_at: grant.authority.expires_at.clone(),
        revoked_at: grant.revoked_at.clone(),
        pending_revocation: grant.pending_revocation.as_ref().map(operation_doc),
        provider_receipt,
        encryption_key_id,
        authority_read_revision: 0,
    })
}

fn read_grant_record(
    operational: &SponsorshipOperationalPseudonymizer,
    document: &GrantDoc,
) -> Result<SponsoredGrant, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsored grant")?;
    if document.authority_read_revision < 0 {
        return Err(SponsorshipStoreError::Malformed(
            "grant authority read revision is negative".into(),
        ));
    }
    if document.provider_receipt.provider_grant_id.key_id() != document.encryption_key_id
        || document.provider_receipt.revocation_handle.key_id() != document.encryption_key_id
    {
        return Err(SponsorshipStoreError::Malformed(
            "grant encryption-key index does not match its receipt envelopes".into(),
        ));
    }
    let grant = SponsoredGrant {
        schema_version: u32::try_from(document.schema_version).map_err(|_| {
            SponsorshipStoreError::Malformed("stored grant schema is invalid".into())
        })?,
        id: SponsoredGrantId::parse(document.id.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        tenant_id: document.tenant_id.clone(),
        policy_id: SponsorshipPolicyId::parse(document.policy_id.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        policy_generation: unsigned(document.policy_generation, "grant policy generation")?,
        credential_generation: unsigned(
            document.credential_generation,
            "grant credential generation",
        )?,
        trigger_revision_id: TriggerRevisionId::parse(document.trigger_revision_id.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        trigger_revision_digest: document.trigger_revision_digest.clone(),
        launch_id: LaunchId::parse(document.launch_id.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        session_id: SessionId(document.session_id.clone()),
        contributor: SubjectId(document.contributor.clone()),
        sponsor_subject: SubjectId(document.sponsor_subject.clone()),
        terms_digest: document.terms_digest.clone(),
        acceptance: document.acceptance.clone(),
        authority: document.authority.clone(),
        capability: document.capability.clone(),
        state: parse_grant_state(&document.state)?,
        allocation: document.allocation.clone(),
        usage: document.usage.clone(),
        generation: unsigned(document.generation, "grant generation")?,
        created_at: document.created_at.clone(),
        updated_at: document.updated_at.clone(),
        revoked_at: document.revoked_at.clone(),
        pending_revocation: document
            .pending_revocation
            .clone()
            .map(read_operation)
            .transpose()?,
    };
    grant
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    for identity in [
        Some(grant.acceptance.idempotency_key_hash.as_str()),
        Some(grant.authority.provider_grant_id_digest.as_str()),
        Some(grant.authority.revocation_handle_digest.as_str()),
        grant
            .pending_revocation
            .as_ref()
            .map(|operation| operation.client_request_id_digest.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        operational
            .parse_configured(identity.to_string())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    }
    let exposure = summary_add(&grant.usage, &document.unresolved_usage)?;
    if !exposure.fits_within(&grant.allocation) {
        return Err(SponsorshipStoreError::Malformed(
            "stored grant usage plus unresolved holds exceeds its allocation".into(),
        ));
    }
    if document.id != grant.id.0
        || document.launch_id != grant.launch_id.0
        || document.contributor != grant.contributor.0
        || document.sponsor_subject.is_empty()
        || document.sponsor_subject.len() > 255
        || document.expires_at != grant.authority.expires_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsored grant indexes do not match the canonical record".into(),
        ));
    }
    Ok(grant)
}

fn require_grant_policy_bindings(
    grant: &SponsoredGrant,
    policy: &SponsorshipPolicy,
) -> Result<(), SponsorshipStoreError> {
    grant
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    policy
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if grant.policy_id != policy.id
        || grant.tenant_id != policy.tenant_id
        || grant.sponsor_subject != *policy.owner.controller()
        || grant.policy_generation > policy.generation
    {
        return Err(SponsorshipStoreError::Malformed(
            "stored grant authority does not match its referenced sponsorship policy".into(),
        ));
    }
    Ok(())
}

fn credential_retirement_covers_activation(
    activation: &GrantActivationIntent,
    policy: &SponsorshipPolicy,
    credential: &SponsoredCredentialRecord,
) -> bool {
    let confirmed_revocation =
        credential
            .last_completed_operation
            .as_ref()
            .is_some_and(|operation| {
                operation.kind == ProviderOperationKind::RevokeCredential
                    && operation.outcome == CompletedProviderOperationOutcome::Confirmed
            });
    credential.state == SponsoredCredentialState::Revoked
        && credential.material_state == SponsoredCredentialMaterialState::Retired
        && credential.revoked_at.is_some()
        && confirmed_revocation
        && policy.id == activation.policy_id
        && policy.generation >= activation.policy_generation
        && policy.credential_id == credential.id
        && credential.generation > activation.credential_generation
        && credential.owner == policy.owner
        && credential.authority.provider == policy.provider_policy.provider
        && credential.authority.provider_policy_key == policy.provider_policy.provider_policy_key
        && activation.allocation.quota == policy.per_launch_quota
        && activation.allocation.spend_micros == policy.per_launch_budget_micros
}

fn read_grant(
    cipher: &SponsorshipCipher,
    operational: &SponsorshipOperationalPseudonymizer,
    document: GrantDoc,
) -> Result<(SponsoredGrant, ProviderGrantReceipt), SponsorshipStoreError> {
    let grant = read_grant_record(operational, &document)?;
    let authority_context = grant_cipher_context(&grant)?;
    let receipt = read_provider_receipt(
        cipher,
        "grant",
        &document.id,
        &authority_context,
        document.provider_receipt,
        &document.encryption_key_id,
    )?;
    require_receipt_matches_grant(operational, &receipt, &grant)?;
    Ok((grant, receipt))
}

fn reservation_doc(
    operational: &SponsorshipOperationalPseudonymizer,
    reservation: &SponsoredUsageReservation,
    request_aliases: &[String],
    policy_id: &SponsorshipPolicyId,
    rate_observed_at: DateTime,
) -> Result<ReservationDoc, SponsorshipStoreError> {
    reservation
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    validate_operational_aliases(operational, request_aliases, "usage request identity")?;
    Ok(ReservationDoc {
        reservation_id: reservation.id.0.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        request_aliases: request_aliases.to_vec(),
        grant_id: reservation.grant_id.0.clone(),
        policy_id: policy_id.0.clone(),
        state: usage_state_code(reservation.state).into(),
        created_at: reservation.created_at.clone(),
        updated_at: reservation.updated_at.clone(),
        rate_observed_at,
        reservation: reservation.clone(),
        credential_id: None,
        model_dispatch_schema_version: None,
        model_dispatch_request_digest: None,
        model_dispatch_payload: None,
        model_response_schema_version: None,
        model_response_digest: None,
        model_response_payload: None,
        encryption_key_id: None,
    })
}

#[allow(clippy::too_many_arguments)] // Every value binds request custody to its public authority.
fn model_reservation_doc(
    cipher: &SponsorshipCipher,
    operational: &SponsorshipOperationalPseudonymizer,
    reservation: &SponsoredUsageReservation,
    request_aliases: &[String],
    policy_id: &SponsorshipPolicyId,
    rate_observed_at: DateTime,
    credential_id: &SponsoredCredentialId,
    dispatch: &SponsoredModelDispatchPayload,
) -> Result<ReservationDoc, SponsorshipStoreError> {
    require_model_dispatch_matches_reservation(dispatch, reservation)?;
    let mut document = reservation_doc(
        operational,
        reservation,
        request_aliases,
        policy_id,
        rate_observed_at,
    )?;
    let context = model_dispatch_cipher_context(
        reservation,
        credential_id,
        dispatch.schema_version(),
        dispatch.request_digest(),
    )?;
    document.model_dispatch_schema_version = Some(i64::from(dispatch.schema_version()));
    document.credential_id = Some(credential_id.0.clone());
    document.model_dispatch_request_digest = Some(dispatch.request_digest().to_string());
    document.model_dispatch_payload = Some(cipher.seal_model_request(
        "usage_reservation",
        &reservation.id.0,
        "model_dispatch_payload",
        &context,
        dispatch.expose_canonical_request_for_encryption(),
    )?);
    document.encryption_key_id = Some(cipher.current_key_id().to_string());
    validate_reservation_custody(&document)?;
    Ok(document)
}

fn replace_reservation_projection(
    mut document: ReservationDoc,
    reservation: &SponsoredUsageReservation,
) -> Result<ReservationDoc, SponsorshipStoreError> {
    reservation
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if document.reservation_id != reservation.id.0
        || document.grant_id != reservation.grant_id.0
        || document.created_at != reservation.created_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "replacement usage reservation changed immutable indexes".into(),
        ));
    }
    document.state = usage_state_code(reservation.state).into();
    document.updated_at = reservation.updated_at.clone();
    document.reservation = reservation.clone();
    if matches!(
        reservation.state,
        SponsoredUsageState::Settled | SponsoredUsageState::Released
    ) {
        document.model_dispatch_schema_version = None;
        document.credential_id = None;
        document.model_dispatch_request_digest = None;
        document.model_dispatch_payload = None;
        document.model_response_schema_version = None;
        document.model_response_digest = None;
        document.model_response_payload = None;
        document.encryption_key_id = None;
    }
    validate_reservation_custody(&document)?;
    Ok(document)
}

fn validate_reservation_custody(document: &ReservationDoc) -> Result<(), SponsorshipStoreError> {
    let request_fields = [
        document.credential_id.is_some(),
        document.model_dispatch_schema_version.is_some(),
        document.model_dispatch_payload.is_some(),
    ];
    let response_fields = [
        document.model_response_schema_version.is_some(),
        document.model_response_digest.is_some(),
        document.model_response_payload.is_some(),
    ];
    if (request_fields.iter().any(|present| *present)
        && request_fields.iter().any(|present| !*present))
        || (response_fields.iter().any(|present| *present)
            && response_fields.iter().any(|present| !*present))
    {
        return Err(SponsorshipStoreError::Malformed(
            "model usage custody metadata is incomplete".into(),
        ));
    }
    let has_request = request_fields[0];
    let has_response = response_fields[0];
    if has_request && has_response {
        return Err(SponsorshipStoreError::Malformed(
            "model request and response custody must be mutually exclusive".into(),
        ));
    }
    let has_payload = has_request || has_response;
    if document.model_dispatch_request_digest.is_some() != has_payload
        || document.encryption_key_id.is_some() != has_payload
    {
        return Err(SponsorshipStoreError::Malformed(
            "model usage shared custody indexes are inconsistent".into(),
        ));
    }
    if document.reservation.state == SponsoredUsageState::Attempting && !has_request {
        return Err(SponsorshipStoreError::Malformed(
            "attempting usage reservation has no model request custody".into(),
        ));
    }
    if has_request
        && document.reservation.state == SponsoredUsageState::Ambiguous
        && (document.reservation.dispatch_revision == 0
            || document.reservation.dispatch_lease.is_some())
    {
        return Err(SponsorshipStoreError::Malformed(
            "ambiguous model usage has no completed dispatch claim".into(),
        ));
    }
    if has_request
        && matches!(
            document.reservation.state,
            SponsoredUsageState::Settled | SponsoredUsageState::Released
        )
    {
        return Err(SponsorshipStoreError::Malformed(
            "terminal usage reservation retained model request custody".into(),
        ));
    }
    if has_response && document.reservation.state != SponsoredUsageState::Settled {
        return Err(SponsorshipStoreError::Malformed(
            "model response custody exists outside settled state".into(),
        ));
    }
    if document.reservation.state == SponsoredUsageState::Released && has_payload {
        return Err(SponsorshipStoreError::Malformed(
            "released model usage retained encrypted payload custody".into(),
        ));
    }
    if has_request {
        let schema_version = document.model_dispatch_schema_version.ok_or_else(|| {
            SponsorshipStoreError::Malformed("model dispatch schema version is missing".into())
        })?;
        if u32::try_from(schema_version).ok()
            != Some(SPONSORED_MODEL_DISPATCH_PAYLOAD_SCHEMA_VERSION)
            || document
                .credential_id
                .as_ref()
                .is_none_or(|id| SponsoredCredentialId::parse(id.clone()).is_err())
            || document
                .model_dispatch_request_digest
                .as_deref()
                .is_none_or(|digest| !is_digest(digest))
            || document
                .model_dispatch_payload
                .as_ref()
                .zip(document.encryption_key_id.as_deref())
                .is_none_or(|(payload, key_id)| payload.key_id() != key_id)
        {
            return Err(SponsorshipStoreError::Malformed(
                "model dispatch custody indexes are invalid".into(),
            ));
        }
    }
    if has_response {
        let schema_version = document.model_response_schema_version.ok_or_else(|| {
            SponsorshipStoreError::Malformed("model response schema version is missing".into())
        })?;
        if u32::try_from(schema_version).ok()
            != Some(SPONSORED_MODEL_RESPONSE_PAYLOAD_SCHEMA_VERSION)
            || document
                .model_dispatch_request_digest
                .as_deref()
                .is_none_or(|digest| !is_digest(digest))
            || document
                .model_response_digest
                .as_deref()
                .is_none_or(|digest| !is_digest(digest))
            || document
                .model_response_payload
                .as_ref()
                .zip(document.encryption_key_id.as_deref())
                .is_none_or(|(payload, key_id)| payload.key_id() != key_id)
            || document.reservation.actual.is_none()
        {
            return Err(SponsorshipStoreError::Malformed(
                "model response custody indexes are invalid".into(),
            ));
        }
    }
    Ok(())
}

fn replace_reservation_for_reconciliation(
    cipher: &SponsorshipCipher,
    document: ReservationDoc,
    reservation: &SponsoredUsageReservation,
    response: Option<&SponsoredModelResponsePayload>,
) -> Result<ReservationDoc, SponsorshipStoreError> {
    validate_reservation_custody(&document)?;
    let had_model_request = document.model_dispatch_payload.is_some();
    let request_digest = if had_model_request {
        Some(
            read_model_dispatch(cipher, &document)?
                .ok_or_else(|| {
                    SponsorshipStoreError::Malformed(
                        "model settlement could not authenticate request custody".into(),
                    )
                })?
                .request_digest()
                .to_string(),
        )
    } else {
        None
    };
    if had_model_request && reservation.state == SponsoredUsageState::Settled && response.is_none()
    {
        return Err(SponsorshipStoreError::Conflict);
    }
    if response.is_some()
        && (!had_model_request || reservation.state != SponsoredUsageState::Settled)
    {
        return Err(SponsorshipStoreError::Conflict);
    }

    let mut replacement = replace_reservation_projection(document, reservation)?;
    if let Some(response) = response {
        require_model_response_matches_reservation(response, reservation)?;
        let request_digest = request_digest.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "model response custody lost its original request digest".into(),
            )
        })?;
        if response.request_digest() != request_digest {
            return Err(SponsorshipStoreError::Conflict);
        }
        let context = model_response_cipher_context(
            reservation,
            response.schema_version(),
            &request_digest,
            response.response_digest(),
        )?;
        replacement.model_dispatch_request_digest = Some(request_digest);
        replacement.model_response_schema_version = Some(i64::from(response.schema_version()));
        replacement.model_response_digest = Some(response.response_digest().to_string());
        replacement.model_response_payload = Some(cipher.seal_model_response(
            "usage_reservation",
            &reservation.id.0,
            "model_response_payload",
            &context,
            response.expose_canonical_response_for_encryption(),
        )?);
        replacement.encryption_key_id = Some(cipher.current_key_id().to_string());
    }
    validate_reservation_custody(&replacement)?;
    Ok(replacement)
}

fn read_model_dispatch(
    cipher: &SponsorshipCipher,
    document: &ReservationDoc,
) -> Result<Option<SponsoredModelDispatchPayload>, SponsorshipStoreError> {
    validate_reservation_custody(document)?;
    let Some(envelope) = document.model_dispatch_payload.as_ref() else {
        return Ok(None);
    };
    let schema_version = u32::try_from(
        document
            .model_dispatch_schema_version
            .ok_or_else(|| SponsorshipStoreError::Malformed("missing dispatch schema".into()))?,
    )
    .map_err(|_| SponsorshipStoreError::Malformed("invalid dispatch schema".into()))?;
    let request_digest = document
        .model_dispatch_request_digest
        .as_deref()
        .ok_or_else(|| SponsorshipStoreError::Malformed("missing dispatch digest".into()))?;
    let context = model_dispatch_cipher_context(
        &document.reservation,
        &SponsoredCredentialId::parse(
            document
                .credential_id
                .clone()
                .ok_or_else(|| SponsorshipStoreError::Malformed("missing credential id".into()))?,
        )
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        schema_version,
        request_digest,
    )?;
    let opened = cipher.open_model_request(
        "usage_reservation",
        &document.reservation_id,
        "model_dispatch_payload",
        &context,
        envelope,
    )?;
    let dispatch = SponsoredModelDispatchPayload::from_encrypted_canonical_request(
        schema_version,
        request_digest,
        opened.plaintext.to_string(),
    )
    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    require_model_dispatch_matches_reservation(&dispatch, &document.reservation)?;
    Ok(Some(dispatch))
}

fn read_model_response(
    cipher: &SponsorshipCipher,
    document: &ReservationDoc,
) -> Result<Option<SponsoredModelResponsePayload>, SponsorshipStoreError> {
    validate_reservation_custody(document)?;
    let Some(envelope) = document.model_response_payload.as_ref() else {
        return Ok(None);
    };
    let schema_version = u32::try_from(
        document
            .model_response_schema_version
            .ok_or_else(|| SponsorshipStoreError::Malformed("missing response schema".into()))?,
    )
    .map_err(|_| SponsorshipStoreError::Malformed("invalid response schema".into()))?;
    let request_digest = document
        .model_dispatch_request_digest
        .as_deref()
        .ok_or_else(|| SponsorshipStoreError::Malformed("missing request digest".into()))?;
    let response_digest = document
        .model_response_digest
        .as_deref()
        .ok_or_else(|| SponsorshipStoreError::Malformed("missing response digest".into()))?;
    let actual_quota = document
        .reservation
        .actual
        .as_ref()
        .ok_or_else(|| SponsorshipStoreError::Malformed("missing settled usage".into()))?
        .quota;
    let context = model_response_cipher_context(
        &document.reservation,
        schema_version,
        request_digest,
        response_digest,
    )?;
    let opened = cipher.open_model_response(
        "usage_reservation",
        &document.reservation_id,
        "model_response_payload",
        &context,
        envelope,
    )?;
    let response = SponsoredModelResponsePayload::from_encrypted_canonical_response(
        schema_version,
        request_digest,
        response_digest,
        actual_quota,
        opened.plaintext.to_string(),
    )
    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    require_model_response_matches_reservation(&response, &document.reservation)?;
    Ok(Some(response))
}

fn require_model_dispatch_matches_reservation(
    dispatch: &SponsoredModelDispatchPayload,
    reservation: &SponsoredUsageReservation,
) -> Result<(), SponsorshipStoreError> {
    dispatch
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let request = dispatch
        .request()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let object = request.as_object().ok_or_else(|| {
        SponsorshipStoreError::Malformed("model dispatch request is not an object".into())
    })?;
    let canonical_bytes = dispatch.expose_canonical_request_for_encryption().len() as u64;
    if reservation.reserved.quota.requests != 1
        || reservation.reserved.quota.input_tokens != canonical_bytes
        || object.get("model").and_then(serde_json::Value::as_str)
            != Some(reservation.price_quote.model.as_str())
        || object
            .get("max_output_tokens")
            .and_then(serde_json::Value::as_u64)
            != Some(reservation.reserved.quota.output_tokens)
        || object.get("stream").is_some_and(|stream| stream != false)
        || reservation.price_quote.resource != layer3_adapter::SPONSORED_MODEL_RESOURCE
    {
        return Err(SponsorshipStoreError::Malformed(
            "model dispatch custody does not match its reservation envelope".into(),
        ));
    }
    Ok(())
}

fn require_model_response_matches_reservation(
    response: &SponsoredModelResponsePayload,
    reservation: &SponsoredUsageReservation,
) -> Result<(), SponsorshipStoreError> {
    response
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if reservation.state != SponsoredUsageState::Settled
        || reservation.actual.as_ref().map(|actual| actual.quota) != Some(response.actual_quota())
        || !response
            .actual_quota()
            .fits_within(reservation.reserved.quota)
    {
        return Err(SponsorshipStoreError::Malformed(
            "model response custody does not match settled accounting".into(),
        ));
    }
    Ok(())
}

fn usage_state_code(state: SponsoredUsageState) -> &'static str {
    match state {
        SponsoredUsageState::Reserved => "reserved",
        SponsoredUsageState::Attempting => "attempting",
        SponsoredUsageState::Ambiguous => "ambiguous",
        SponsoredUsageState::Settled => "settled",
        SponsoredUsageState::Released => "released",
    }
}

fn read_reservation(
    operational: &SponsorshipOperationalPseudonymizer,
    document: ReservationDoc,
) -> Result<(SponsoredUsageReservation, SponsorshipPolicyId, DateTime), SponsorshipStoreError> {
    validate_document_version(
        document.document_schema_version,
        "sponsored usage reservation",
    )?;
    validate_reservation_custody(&document)?;
    document
        .reservation
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    validate_operational_aliases(
        operational,
        &document.request_aliases,
        "usage request identity",
    )?;
    if document.reservation_id != document.reservation.id.0
        || document.grant_id != document.reservation.grant_id.0
        || document.state != usage_state_code(document.reservation.state)
        || document.created_at != document.reservation.created_at
        || document.updated_at != document.reservation.updated_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsored usage indexes do not match the canonical reservation".into(),
        ));
    }
    let policy_id = SponsorshipPolicyId::parse(document.policy_id)
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok((document.reservation, policy_id, document.rate_observed_at))
}

fn audit_doc(event: &SponsoredAuditEvent) -> Result<AuditDoc, SponsorshipStoreError> {
    validate_audit_event(event)?;
    Ok(AuditDoc {
        event_id: event.event_id.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        actor_digest: event.actor_digest.clone(),
        policy_id: event.policy_id.as_ref().map(|id| id.0.clone()),
        grant_id: event.grant_id.as_ref().map(|id| id.0.clone()),
        occurred_at: event.occurred_at.clone(),
        event: event.clone(),
    })
}

fn read_audit(document: AuditDoc) -> Result<SponsoredAuditEvent, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship audit event")?;
    validate_audit_event(&document.event)?;
    if document.event_id != document.event.event_id
        || document.actor_digest != document.event.actor_digest
        || document.policy_id != document.event.policy_id.as_ref().map(|id| id.0.clone())
        || document.grant_id != document.event.grant_id.as_ref().map(|id| id.0.clone())
        || document.occurred_at != document.event.occurred_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship audit indexes do not match the canonical event".into(),
        ));
    }
    Ok(document.event)
}

fn malformed_code(field: &str, value: &str) -> SponsorshipStoreError {
    SponsorshipStoreError::Malformed(format!("stored {field} code '{value}' is unsupported"))
}

fn validate_audit_event(event: &SponsoredAuditEvent) -> Result<(), SponsorshipStoreError> {
    let event_id = uuid::Uuid::parse_str(&event.event_id).map_err(|_| {
        SponsorshipStoreError::Malformed("sponsorship audit event id is not a UUID".into())
    })?;
    if event_id.to_string() != event.event_id
        || !is_digest(&event.actor_digest)
        || event
            .stable_request_id_digest
            .as_ref()
            .is_some_and(|digest| !is_digest(digest))
        || event.outcome_code.is_empty()
        || event.outcome_code.len() > 64
        || !event
            .outcome_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
        || layer2_core::seconds_from_rfc3339(&event.occurred_at).is_none()
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship audit event contains malformed identity, digest, outcome, or time".into(),
        ));
    }
    if let Some(policy_id) = &event.policy_id {
        SponsorshipPolicyId::parse(policy_id.0.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    }
    if let Some(grant_id) = &event.grant_id {
        SponsoredGrantId::parse(grant_id.0.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    }
    Ok(())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_operational_aliases(
    pseudonymizer: &SponsorshipOperationalPseudonymizer,
    aliases: &[String],
    kind: &str,
) -> Result<BTreeSet<String>, SponsorshipStoreError> {
    if aliases.is_empty() || aliases.len() > MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS + 1 {
        return Err(SponsorshipStoreError::Malformed(format!(
            "stored {kind} aliases are empty or exceed the bounded keyring"
        )));
    }
    let mut values = BTreeSet::new();
    let mut key_ids = BTreeSet::new();
    for encoded in aliases {
        let alias = pseudonymizer
            .parse_configured(encoded.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if !values.insert(alias.as_str().to_string()) || !key_ids.insert(alias.key_id().to_string())
        {
            return Err(SponsorshipStoreError::Malformed(format!(
                "stored {kind} aliases contain a duplicate value or key id"
            )));
        }
    }
    Ok(values)
}

fn validate_credential_provider_aliases(
    pseudonymizer: &SponsorshipOperationalPseudonymizer,
    document: &CredentialDoc,
) -> Result<(), SponsorshipStoreError> {
    let aliases = validate_operational_aliases(
        pseudonymizer,
        &document.provider_credential_id_aliases,
        "provider credential identity",
    )?;
    let canonical = pseudonymizer
        .parse_configured(document.provider_credential_id_digest.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if !aliases.contains(canonical.as_str()) {
        return Err(SponsorshipStoreError::Malformed(
            "provider credential aliases omit the canonical authority identity".into(),
        ));
    }
    Ok(())
}

fn validate_credential_operation_dependencies(
    pseudonymizer: &SponsorshipOperationalPseudonymizer,
    document: &CredentialDoc,
) -> Result<(), SponsorshipStoreError> {
    for client_request_id in document
        .pending_operation
        .as_ref()
        .map(|operation| operation.client_request_id_digest.as_str())
        .into_iter()
        .chain(
            document
                .superseded_operation
                .as_ref()
                .map(|operation| operation.client_request_id_digest.as_str()),
        )
        .chain(
            document
                .last_completed_operation
                .as_ref()
                .map(|operation| operation.client_request_id_digest.as_str()),
        )
    {
        pseudonymizer
            .parse_configured(client_request_id.to_string())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    }
    Ok(())
}

fn validate_credential_registration_doc(
    pseudonymizer: &SponsorshipOperationalPseudonymizer,
    document: &CredentialRegistrationDoc,
) -> Result<(), SponsorshipStoreError> {
    if document.document_schema_version != CREDENTIAL_REGISTRATION_DOCUMENT_SCHEMA_VERSION {
        return Err(SponsorshipStoreError::Malformed(format!(
            "stored credential registration document schema {} is unsupported",
            document.document_schema_version
        )));
    }
    let parsed_id = Uuid::parse_str(&document.id).map_err(|_| {
        SponsorshipStoreError::Malformed("stored credential registration id is malformed".into())
    })?;
    if parsed_id.to_string() != document.id {
        return Err(SponsorshipStoreError::Malformed(
            "stored credential registration id is not canonical".into(),
        ));
    }
    SponsoredCredentialId::parse(document.credential_id.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    validate_operational_aliases(
        pseudonymizer,
        &document.request_aliases,
        "credential registration request",
    )?;
    validate_operational_aliases(
        pseudonymizer,
        &document.command_aliases,
        "credential registration command",
    )?;
    if layer2_core::seconds_from_rfc3339(&document.created_at).is_none() {
        return Err(SponsorshipStoreError::Malformed(
            "stored credential registration time is malformed".into(),
        ));
    }
    Ok(())
}

fn validate_policy_creation_doc(
    pseudonymizer: &SponsorshipOperationalPseudonymizer,
    document: &PolicyCreationDoc,
) -> Result<(), SponsorshipStoreError> {
    if document.document_schema_version != POLICY_CREATION_DOCUMENT_SCHEMA_VERSION {
        return Err(SponsorshipStoreError::Malformed(format!(
            "stored policy creation document schema {} is unsupported",
            document.document_schema_version
        )));
    }
    let parsed_id = Uuid::parse_str(&document.id).map_err(|_| {
        SponsorshipStoreError::Malformed("stored policy creation id is malformed".into())
    })?;
    if parsed_id.to_string() != document.id {
        return Err(SponsorshipStoreError::Malformed(
            "stored policy creation id is not canonical".into(),
        ));
    }
    SponsorshipPolicyId::parse(document.policy_id.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    validate_operational_aliases(
        pseudonymizer,
        &document.request_aliases,
        "policy creation request",
    )?;
    validate_operational_aliases(
        pseudonymizer,
        &document.command_aliases,
        "policy creation command",
    )?;
    if !is_digest(&document.payload_digest) {
        return Err(SponsorshipStoreError::Malformed(
            "stored policy creation payload digest is malformed".into(),
        ));
    }
    if layer2_core::seconds_from_rfc3339(&document.created_at).is_none() {
        return Err(SponsorshipStoreError::Malformed(
            "stored policy creation time is malformed".into(),
        ));
    }
    Ok(())
}

fn policy_creation_doc(
    operational: &SponsorshipOperationalPseudonymizer,
    identity: &SponsorshipPolicyCreationIdentity,
    policy_id: &SponsorshipPolicyId,
    created_at: &str,
) -> Result<PolicyCreationDoc, SponsorshipStoreError> {
    validate_operational_aliases(
        operational,
        &identity.request.encoded_candidates(),
        "policy creation request",
    )?;
    validate_operational_aliases(
        operational,
        &identity.command.encoded_candidates(),
        "policy creation command",
    )?;
    if !is_digest(&identity.payload_digest) {
        return Err(SponsorshipStoreError::Malformed(
            "policy creation payload digest is malformed".into(),
        ));
    }
    SponsorshipPolicyId::parse(policy_id.0.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if layer2_core::seconds_from_rfc3339(created_at).is_none() {
        return Err(SponsorshipStoreError::Malformed(
            "policy creation time is malformed".into(),
        ));
    }
    Ok(PolicyCreationDoc {
        id: Uuid::new_v4().to_string(),
        document_schema_version: POLICY_CREATION_DOCUMENT_SCHEMA_VERSION,
        request_aliases: identity.request.encoded_candidates(),
        command_aliases: identity.command.encoded_candidates(),
        payload_digest: identity.payload_digest.clone(),
        policy_id: policy_id.0.clone(),
        created_at: created_at.to_string(),
    })
}

fn aliases_match_candidates(
    aliases: &[String],
    candidates: &SponsorshipOperationalPseudonymSet,
) -> bool {
    candidates
        .iter()
        .any(|candidate| aliases.iter().any(|alias| alias == candidate.as_str()))
}

fn credential_registration_doc(
    identity: &SponsoredCredentialRegistrationIdentity,
    credential_id: &SponsoredCredentialId,
    created_at: &str,
) -> CredentialRegistrationDoc {
    CredentialRegistrationDoc {
        id: Uuid::new_v4().to_string(),
        document_schema_version: CREDENTIAL_REGISTRATION_DOCUMENT_SCHEMA_VERSION,
        request_aliases: identity.request.encoded_candidates(),
        command_aliases: identity.command.encoded_candidates(),
        credential_id: credential_id.0.clone(),
        created_at: created_at.to_string(),
    }
}

fn credential_registration_bindings_match(
    stored: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    stored.owner == candidate.owner
        && stored.authority.provider == candidate.authority.provider
        && stored.authority.provider_policy_key == candidate.authority.provider_policy_key
        && stored.authority.supports_expiry == candidate.authority.supports_expiry
        && stored.authority.supports_revocation == candidate.authority.supports_revocation
}

fn credential_lineage_matches(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    current.id == candidate.id
        && current.owner == candidate.owner
        && current.authority.provider == candidate.authority.provider
        && current.authority.provider_policy_key == candidate.authority.provider_policy_key
        && current.created_at == candidate.created_at
}

fn credential_authority_matches(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    credential_lineage_matches(current, candidate) && current.authority == candidate.authority
}

fn credential_phase_update_is_legal(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    if current.state != candidate.state
        || !matches!(
            current.state,
            SponsoredCredentialState::RotationPending | SponsoredCredentialState::RevocationPending
        )
        || current.material_state != SponsoredCredentialMaterialState::Retained
        || candidate.material_state != SponsoredCredentialMaterialState::Retained
        || !credential_authority_matches(current, candidate)
        || current.revoked_at != candidate.revoked_at
        || current.superseded_operation != candidate.superseded_operation
        || current.last_completed_operation != candidate.last_completed_operation
        || candidate.updated_at < current.updated_at
    {
        return false;
    }
    let (Some(current_operation), Some(candidate_operation)) = (
        current.pending_operation.as_ref(),
        candidate.pending_operation.as_ref(),
    ) else {
        return false;
    };
    operation_lineage_matches(current_operation, candidate_operation)
        && operation_phase_transition_is_legal(current_operation.phase, candidate_operation.phase)
        && (current_operation.attempted_at.is_none()
            || candidate_operation.phase == ProviderOperationPhase::Prepared
            || current_operation.attempted_at == candidate_operation.attempted_at)
}

fn credential_emergency_revocation_is_legal(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    if !matches!(
        current.state,
        SponsoredCredentialState::Active | SponsoredCredentialState::RotationPending
    ) || candidate.state != SponsoredCredentialState::RevocationPending
        || current.material_state != SponsoredCredentialMaterialState::Retained
        || candidate.material_state != SponsoredCredentialMaterialState::Retained
        || !credential_authority_matches(current, candidate)
        || candidate.revoked_at.is_some()
        || current.last_completed_operation != candidate.last_completed_operation
        || candidate.updated_at < current.updated_at
    {
        return false;
    }
    let Some(operation) = candidate.pending_operation.as_ref() else {
        return false;
    };
    if operation.kind != ProviderOperationKind::RevokeCredential
        || operation.phase != ProviderOperationPhase::Prepared
        || operation.attempted_at.is_some()
        || operation.requested_at < current.updated_at
    {
        return false;
    }
    match current.state {
        SponsoredCredentialState::Active => {
            current.pending_operation.is_none()
                && current.superseded_operation.is_none()
                && candidate.superseded_operation.is_none()
        }
        SponsoredCredentialState::RotationPending => {
            current.pending_operation.is_some()
                && candidate.superseded_operation == current.pending_operation
        }
        SponsoredCredentialState::RevocationPending | SponsoredCredentialState::Revoked => false,
    }
}

fn credential_update_is_legal(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    credential_phase_update_is_legal(current, candidate)
        || credential_emergency_revocation_is_legal(current, candidate)
}

fn credential_revocation_attempt_begins(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    if current.state != SponsoredCredentialState::RevocationPending
        || candidate.state != SponsoredCredentialState::RevocationPending
    {
        return false;
    }
    matches!(
        (
            current.pending_operation.as_ref(),
            candidate.pending_operation.as_ref(),
        ),
        (
            Some(PendingProviderOperation {
                kind: ProviderOperationKind::RevokeCredential,
                phase: ProviderOperationPhase::Prepared,
                ..
            }),
            Some(PendingProviderOperation {
                kind: ProviderOperationKind::RevokeCredential,
                phase: ProviderOperationPhase::Attempting,
                ..
            }),
        )
    )
}

fn credential_rotation_stage_is_legal(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    if current.state != SponsoredCredentialState::Active
        || candidate.state != SponsoredCredentialState::RotationPending
        || current.material_state != SponsoredCredentialMaterialState::Retained
        || candidate.material_state != SponsoredCredentialMaterialState::Retained
        || !credential_authority_matches(current, candidate)
        || current.pending_operation.is_some()
        || current.superseded_operation.is_some()
        || candidate.superseded_operation.is_some()
        || current.last_completed_operation != candidate.last_completed_operation
        || current.revoked_at.is_some()
        || candidate.revoked_at.is_some()
        || candidate.updated_at < current.updated_at
    {
        return false;
    }
    candidate
        .pending_operation
        .as_ref()
        .is_some_and(|operation| {
            operation.kind == ProviderOperationKind::RotateCredential
                && operation.phase == ProviderOperationPhase::Prepared
                && operation.attempted_at.is_none()
                && operation.requested_at >= current.updated_at
        })
}

fn policy_lineage_matches(current: &SponsorshipPolicy, candidate: &SponsorshipPolicy) -> bool {
    current.id == candidate.id
        && current.tenant_id == candidate.tenant_id
        && current.owner == candidate.owner
        && current.credential_id == candidate.credential_id
        && current.provider_policy == candidate.provider_policy
        && current.per_launch_quota == candidate.per_launch_quota
        && current.aggregate_quota == candidate.aggregate_quota
        && current.per_launch_budget_micros == candidate.per_launch_budget_micros
        && current.aggregate_budget_micros == candidate.aggregate_budget_micros
        && current.max_requests_per_minute == candidate.max_requests_per_minute
        && current.max_concurrent_launches == candidate.max_concurrent_launches
        && current.expires_at == candidate.expires_at
        && current.created_at == candidate.created_at
}

fn policy_update_is_legal(current: &SponsorshipPolicy, candidate: &SponsorshipPolicy) -> bool {
    if !policy_lineage_matches(current, candidate) || candidate.updated_at < current.updated_at {
        return false;
    }
    match (current.state, candidate.state) {
        (
            layer2_core::SponsorshipPolicyState::Active,
            layer2_core::SponsorshipPolicyState::Suspended,
        ) => candidate.revoked_at.is_none(),
        (
            layer2_core::SponsorshipPolicyState::Active
            | layer2_core::SponsorshipPolicyState::Suspended,
            layer2_core::SponsorshipPolicyState::Revoked,
        ) => candidate.revoked_at.as_deref() == Some(candidate.updated_at.as_str()),
        _ => false,
    }
}

fn grant_lineage_matches(current: &SponsoredGrant, candidate: &SponsoredGrant) -> bool {
    current.id == candidate.id
        && current.tenant_id == candidate.tenant_id
        && current.policy_id == candidate.policy_id
        && current.policy_generation == candidate.policy_generation
        && current.credential_generation == candidate.credential_generation
        && current.trigger_revision_id == candidate.trigger_revision_id
        && current.trigger_revision_digest == candidate.trigger_revision_digest
        && current.launch_id == candidate.launch_id
        && current.session_id == candidate.session_id
        && current.contributor == candidate.contributor
        && current.sponsor_subject == candidate.sponsor_subject
        && current.terms_digest == candidate.terms_digest
        && current.acceptance == candidate.acceptance
        && current.authority == candidate.authority
        && current.created_at == candidate.created_at
}

fn grant_update_is_legal(current: &SponsoredGrant, candidate: &SponsoredGrant) -> bool {
    if !grant_lineage_matches(current, candidate)
        || current.usage != candidate.usage
        || current.allocation != candidate.allocation
        || current.revoked_at != candidate.revoked_at
        || candidate.updated_at < current.updated_at
    {
        return false;
    }
    match (
        current.state,
        candidate.state,
        &current.pending_revocation,
        &candidate.pending_revocation,
    ) {
        (
            SponsoredGrantState::Active | SponsoredGrantState::Suspended,
            SponsoredGrantState::RevocationPending,
            None,
            Some(operation),
        ) => {
            matches!(
                operation.kind,
                ProviderOperationKind::RevokeGrant | ProviderOperationKind::CloseGrant
            ) && operation.phase == ProviderOperationPhase::Prepared
                && operation.attempted_at.is_none()
                && operation.requested_at >= current.updated_at
        }
        (
            SponsoredGrantState::RevocationPending,
            SponsoredGrantState::RevocationPending,
            Some(current_operation),
            Some(candidate_operation),
        ) => {
            operation_lineage_matches(current_operation, candidate_operation)
                && operation_phase_transition_is_legal(
                    current_operation.phase,
                    candidate_operation.phase,
                )
                && (current_operation.attempted_at.is_none()
                    || candidate_operation.phase == ProviderOperationPhase::Prepared
                    || current_operation.attempted_at == candidate_operation.attempted_at)
        }
        _ => false,
    }
}

fn activation_lineage_matches(
    current: &GrantActivationIntent,
    candidate: &GrantActivationIntent,
) -> bool {
    current.launch_id == candidate.launch_id
        && current.session_id == candidate.session_id
        && current.policy_id == candidate.policy_id
        && current.policy_generation == candidate.policy_generation
        && current.credential_generation == candidate.credential_generation
        && current.launch_authority_digest == candidate.launch_authority_digest
        && current.acceptance_digest == candidate.acceptance_digest
        && current.gate_fence == candidate.gate_fence
        && current.allocation == candidate.allocation
        && current.request_fingerprint == candidate.request_fingerprint
        && current.billing_request_id == candidate.billing_request_id
        && current.provider_request_id == candidate.provider_request_id
        && current.created_at == candidate.created_at
}

fn activation_claim_bindings_match(
    current: &GrantActivationIntent,
    candidate: &GrantActivationIntent,
    billing_request_candidates: &SponsorshipOperationalPseudonymSet,
    provider_request_candidates: &SponsorshipOperationalPseudonymSet,
) -> bool {
    current.launch_id == candidate.launch_id
        && current.session_id == candidate.session_id
        && current.policy_id == candidate.policy_id
        && current.policy_generation == candidate.policy_generation
        && current.credential_generation == candidate.credential_generation
        && current.launch_authority_digest == candidate.launch_authority_digest
        && current.acceptance_digest == candidate.acceptance_digest
        && current.gate_fence == candidate.gate_fence
        && current.allocation == candidate.allocation
        && current.request_fingerprint == candidate.request_fingerprint
        && billing_request_candidates
            .iter()
            .any(|identity| identity.as_str() == current.billing_request_id)
        && provider_request_candidates
            .iter()
            .any(|identity| identity.as_str() == current.provider_request_id)
}

fn operation_lineage_matches(
    current: &PendingProviderOperation,
    candidate: &PendingProviderOperation,
) -> bool {
    current.kind == candidate.kind
        && current.provider_request_id == candidate.provider_request_id
        && current.client_request_id_digest == candidate.client_request_id_digest
        && current.payload_digest == candidate.payload_digest
        && current.requested_at == candidate.requested_at
}

fn operation_phase_transition_is_legal(
    current: ProviderOperationPhase,
    candidate: ProviderOperationPhase,
) -> bool {
    matches!(
        (current, candidate),
        (
            ProviderOperationPhase::Prepared,
            ProviderOperationPhase::Attempting
        ) | (
            ProviderOperationPhase::Attempting,
            ProviderOperationPhase::Prepared
        ) | (
            ProviderOperationPhase::Attempting | ProviderOperationPhase::Ambiguous,
            ProviderOperationPhase::Ambiguous | ProviderOperationPhase::Rejected
        )
    )
}

fn activation_update_is_legal(
    current: &GrantActivationIntent,
    candidate: &GrantActivationIntent,
) -> bool {
    if !activation_lineage_matches(current, candidate)
        || candidate.revision != current.revision.saturating_add(1)
        || candidate.updated_at < current.updated_at
        || candidate
            .lease
            .as_ref()
            .is_some_and(|lease| current.lease.as_ref() != Some(lease))
        || (!current.capacity_held && candidate.capacity_held)
    {
        return false;
    }
    match (&current.provider_receipt, &candidate.provider_receipt) {
        (Some(current), Some(candidate)) if current != candidate => return false,
        (Some(_), None) => return false,
        (None, Some(_))
            if !matches!(
                (current.state, candidate.state),
                (
                    GrantActivationState::ProviderAttempting
                        | GrantActivationState::ProviderAmbiguous,
                    GrantActivationState::ProviderConfirmed
                        | GrantActivationState::ProviderAmbiguous
                )
            ) =>
        {
            return false;
        }
        _ => {}
    }
    match (&current.cleanup_operation, &candidate.cleanup_operation) {
        (None, None) => {}
        (Some(current_operation), Some(candidate_operation))
            if current.state == GrantActivationState::CleanupPending
                && candidate.state == GrantActivationState::CleanupPending
                && operation_lineage_matches(current_operation, candidate_operation)
                && operation_phase_transition_is_legal(
                    current_operation.phase,
                    candidate_operation.phase,
                )
                && (current_operation.attempted_at.is_none()
                    || candidate_operation.phase == ProviderOperationPhase::Prepared
                    || current_operation.attempted_at == candidate_operation.attempted_at) => {}
        _ => return false,
    }
    matches!(
        (current.state, candidate.state, candidate.denial),
        (
            GrantActivationState::AwaitingBilling,
            GrantActivationState::BillingAttempting,
            None
        ) | (
            GrantActivationState::BillingAttempting,
            GrantActivationState::AwaitingBilling | GrantActivationState::BillingAmbiguous,
            None
        ) | (
            GrantActivationState::BillingAmbiguous,
            GrantActivationState::BillingAmbiguous,
            None
        ) | (
            GrantActivationState::BillingAttempting | GrantActivationState::BillingAmbiguous,
            GrantActivationState::Denied,
            Some(GrantActivationDenial::BillingRejected)
        ) | (
            GrantActivationState::ProviderAttempting,
            GrantActivationState::AwaitingProvider
                | GrantActivationState::ProviderAmbiguous
                | GrantActivationState::ProviderConfirmed,
            None
        ) | (
            GrantActivationState::ProviderAmbiguous,
            GrantActivationState::ProviderAmbiguous | GrantActivationState::ProviderConfirmed,
            None
        ) | (
            GrantActivationState::ProviderAttempting | GrantActivationState::ProviderAmbiguous,
            GrantActivationState::Denied,
            Some(GrantActivationDenial::ProviderRejected)
        ) | (
            GrantActivationState::CleanupPending,
            GrantActivationState::CleanupPending,
            None
        )
    )
}

fn require_successor_generation(
    expected: u64,
    candidate: u64,
    resource: &str,
) -> Result<(), SponsorshipStoreError> {
    let successor = expected.checked_add(1).ok_or_else(|| {
        SponsorshipStoreError::Malformed(format!("{resource} generation is exhausted"))
    })?;
    if candidate != successor {
        return Err(SponsorshipStoreError::StaleGeneration);
    }
    Ok(())
}

fn credential_operation_completion(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
    kind: ProviderOperationKind,
    outcome: CompletedProviderOperationOutcome,
) -> Result<CompletedProviderOperation, SponsorshipStoreError> {
    let pending = current
        .pending_operation
        .as_ref()
        .ok_or(SponsorshipStoreError::Conflict)?;
    let completed = candidate
        .last_completed_operation
        .as_ref()
        .ok_or(SponsorshipStoreError::Conflict)?;
    if !matches!(
        pending.phase,
        ProviderOperationPhase::Attempting | ProviderOperationPhase::Ambiguous
    ) || pending.kind != kind
        || completed.kind != kind
        || completed.outcome != outcome
        || pending.provider_request_id != completed.provider_request_id
        || pending.client_request_id_digest != completed.client_request_id_digest
        || pending.payload_digest != completed.payload_digest
        || current.id != candidate.id
        || candidate.updated_at < current.updated_at
        || completed.completed_at < pending.requested_at
        || pending
            .attempted_at
            .as_ref()
            .is_some_and(|attempted_at| completed.completed_at < *attempted_at)
        || completed.completed_at < current.updated_at
        || candidate.updated_at != completed.completed_at
    {
        return Err(SponsorshipStoreError::Conflict);
    }
    completed
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(completed.clone())
}

fn credential_completion_shape_is_legal(
    current: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
    kind: ProviderOperationKind,
    outcome: CompletedProviderOperationOutcome,
) -> bool {
    let Some(pending) = current.pending_operation.as_ref() else {
        return false;
    };
    if !credential_lineage_matches(current, candidate)
        || current.material_state != SponsoredCredentialMaterialState::Retained
        || candidate.pending_operation.is_some()
        || candidate.superseded_operation.is_some()
    {
        return false;
    }
    match (kind, outcome) {
        (ProviderOperationKind::RotateCredential, CompletedProviderOperationOutcome::Confirmed) => {
            current.state == SponsoredCredentialState::RotationPending
                && candidate.state == SponsoredCredentialState::Active
                && candidate.material_state == SponsoredCredentialMaterialState::Retained
                && candidate.revoked_at.is_none()
                && candidate.authority.provider == current.authority.provider
                && candidate.authority.provider_policy_key == current.authority.provider_policy_key
                && candidate.authority.provider_credential_id_digest
                    != current.authority.provider_credential_id_digest
                && candidate.authority.state_version > current.authority.state_version
                && candidate.authority.supports_expiry == current.authority.supports_expiry
                && candidate.authority.supports_revocation == current.authority.supports_revocation
                && candidate.authority.ownership_verified_at >= pending.requested_at
                && pending.attempted_at.as_ref().is_none_or(|attempted_at| {
                    candidate.authority.ownership_verified_at >= *attempted_at
                })
                && candidate
                    .last_completed_operation
                    .as_ref()
                    .is_some_and(|completed| {
                        candidate.authority.ownership_verified_at <= completed.completed_at
                    })
        }
        (ProviderOperationKind::RotateCredential, CompletedProviderOperationOutcome::Rejected) => {
            current.state == SponsoredCredentialState::RotationPending
                && candidate.state == SponsoredCredentialState::Active
                && candidate.material_state == SponsoredCredentialMaterialState::Retained
                && candidate.authority == current.authority
                && candidate.revoked_at.is_none()
        }
        (ProviderOperationKind::RevokeCredential, CompletedProviderOperationOutcome::Confirmed) => {
            current.state == SponsoredCredentialState::RevocationPending
                && candidate.state == SponsoredCredentialState::Revoked
                && candidate.material_state == SponsoredCredentialMaterialState::Retired
                && candidate.authority == current.authority
                && candidate.revoked_at.as_deref() == Some(candidate.updated_at.as_str())
        }
        _ => false,
    }
}

fn ledger_doc(
    credential_id: &SponsoredCredentialId,
    operation: &CompletedProviderOperation,
) -> CredentialOperationLedgerDoc {
    CredentialOperationLedgerDoc {
        id: CredentialOperationLedgerId {
            credential_id: credential_id.0.clone(),
            operation_kind: operation_kind_code(operation.kind).into(),
            client_request_id_digest: operation.client_request_id_digest.clone(),
        },
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        operation: completed_operation_doc(operation),
    }
}

fn read_ledger(
    document: CredentialOperationLedgerDoc,
) -> Result<CompletedProviderOperation, SponsorshipStoreError> {
    validate_document_version(
        document.document_schema_version,
        "credential operation replay",
    )?;
    let operation = read_completed_operation(document.operation)?;
    if document.id.operation_kind != operation_kind_code(operation.kind)
        || document.id.client_request_id_digest != operation.client_request_id_digest
    {
        return Err(SponsorshipStoreError::Malformed(
            "credential operation replay key does not match its outcome".into(),
        ));
    }
    SponsoredCredentialId::parse(document.id.credential_id)
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(operation)
}

fn acceptance_payload_fields_match(
    stored: &SponsoredGrantAcceptance,
    candidate: &SponsoredGrantAcceptance,
) -> bool {
    stored.launch_id == candidate.launch_id
        && stored.contributor == candidate.contributor
        && stored.revision_id == candidate.revision_id
        && stored.revision_digest == candidate.revision_digest
        && stored.policy_id == candidate.policy_id
        && stored.policy_generation == candidate.policy_generation
        && stored.terms_digest == candidate.terms_digest
        && stored.launch_snapshot_digest == candidate.launch_snapshot_digest
}

fn acceptance_identity_matches(
    stored: &SponsoredGrantAcceptance,
    candidates: &SponsorshipOperationalPseudonymSet,
) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.as_str() == stored.idempotency_key_hash)
}

fn launch_sponsorship_policy_id(
    launch: &Launch,
) -> Result<SponsorshipPolicyId, SponsorshipStoreError> {
    launch
        .revision
        .content
        .commercial_policy
        .sponsorship
        .as_ref()
        .map(|binding| binding.policy_id.clone())
        .ok_or(SponsorshipStoreError::LaunchAuthorityChanged)
}

fn database_time_string(now: DateTime) -> String {
    layer2_core::rfc3339_at(now.to_system_time())
}

fn summary_sub(
    left: &layer2_core::SponsoredUsageSummary,
    right: &layer2_core::SponsoredUsageSummary,
) -> Result<layer2_core::SponsoredUsageSummary, SponsorshipStoreError> {
    left.checked_sub(right)
        .ok_or(SponsorshipStoreError::Conflict)
}

fn summary_add(
    left: &layer2_core::SponsoredUsageSummary,
    right: &layer2_core::SponsoredUsageSummary,
) -> Result<layer2_core::SponsoredUsageSummary, SponsorshipStoreError> {
    left.checked_add(right)
        .ok_or(SponsorshipStoreError::Conflict)
}

fn database_time_seconds(now: DateTime) -> Result<u64, SponsorshipStoreError> {
    layer2_core::seconds_from_rfc3339(&database_time_string(now)).ok_or_else(|| {
        SponsorshipStoreError::Malformed("the sponsorship database clock is invalid".into())
    })
}

fn timestamp_seconds(value: &str, field: &str) -> Result<u64, SponsorshipStoreError> {
    layer2_core::seconds_from_rfc3339(value)
        .ok_or_else(|| SponsorshipStoreError::Malformed(format!("stored {field} is invalid")))
}

fn timestamp_date_time(value: &str, field: &str) -> Result<DateTime, SponsorshipStoreError> {
    let millis = timestamp_seconds(value, field)?
        .checked_mul(1_000)
        .and_then(|value| i64::try_from(value).ok())
        .ok_or_else(|| SponsorshipStoreError::Malformed(format!("{field} is out of range")))?;
    Ok(DateTime::from_millis(millis))
}

fn later_timestamp(current: &str, observed: &str) -> Result<String, SponsorshipStoreError> {
    let current_seconds = timestamp_seconds(current, "current record update time")?;
    let observed_seconds = timestamp_seconds(observed, "observed record update time")?;
    Ok(if current_seconds > observed_seconds {
        current.to_string()
    } else {
        observed.to_string()
    })
}

fn activation_lease(
    holder: String,
    now: DateTime,
) -> Result<(GrantActivationLease, DateTime), SponsorshipStoreError> {
    let expires_millis = now
        .timestamp_millis()
        .checked_add(ACTIVATION_LEASE_MILLIS)
        .ok_or_else(|| {
            SponsorshipStoreError::Malformed("grant activation lease time overflowed".into())
        })?;
    let expires_at_database = DateTime::from_millis(expires_millis);
    let lease = GrantActivationLease {
        holder,
        acquired_at: database_time_string(now),
        expires_at: database_time_string(expires_at_database),
    };
    lease
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok((lease, expires_at_database))
}

fn usage_dispatch_lease(
    holder: String,
    now: DateTime,
) -> Result<SponsoredUsageDispatchLease, SponsorshipStoreError> {
    let expires_millis = now
        .timestamp_millis()
        .checked_add(USAGE_DISPATCH_LEASE_MILLIS)
        .ok_or_else(|| {
            SponsorshipStoreError::Malformed("usage dispatch lease time overflowed".into())
        })?;
    let lease = SponsoredUsageDispatchLease {
        holder,
        acquired_at: database_time_string(now),
        expires_at: database_time_string(DateTime::from_millis(expires_millis)),
    };
    lease
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(lease)
}

fn validate_activation_lease_database_binding(
    lease: &GrantActivationLease,
    expires_at_database: DateTime,
) -> Result<(), SponsorshipStoreError> {
    if database_time_string(expires_at_database) != lease.expires_at {
        return Err(SponsorshipStoreError::Malformed(
            "activation lease database expiry does not match its public expiry".into(),
        ));
    }
    let acquired = timestamp_seconds(&lease.acquired_at, "activation lease acquisition")?;
    let expires = timestamp_seconds(&lease.expires_at, "activation lease expiry")?;
    let expected_duration = u64::try_from(ACTIVATION_LEASE_MILLIS / 1_000).map_err(|_| {
        SponsorshipStoreError::Malformed("activation lease duration is invalid".into())
    })?;
    if expires.checked_sub(acquired) != Some(expected_duration) {
        return Err(SponsorshipStoreError::Malformed(
            "activation lease does not have the required fixed duration".into(),
        ));
    }
    Ok(())
}

fn grant_capacity_is_released(state: SponsoredGrantState) -> bool {
    matches!(
        state,
        SponsoredGrantState::Exhausted
            | SponsoredGrantState::Expired
            | SponsoredGrantState::Revoked
            | SponsoredGrantState::Closed
    )
}

fn deterministic_grant_id_matches(id: &SponsoredGrantId, provider_request_id: &str) -> bool {
    let digest = Sha256::digest(
        [
            b"cma-sponsored-grant-id-v1:".as_slice(),
            provider_request_id.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    id.0 == Uuid::from_bytes(bytes).to_string()
}

fn canonical_cipher_context<T: Serialize + ?Sized>(
    domain: &str,
    authority: &T,
) -> Result<String, SponsorshipStoreError> {
    let encoded = serde_json::to_vec(&(domain, authority)).map_err(|_| {
        SponsorshipStoreError::Malformed(
            "canonical sponsorship ciphertext authority cannot be encoded".into(),
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(encoded)))
}

#[allow(clippy::too_many_arguments)] // Every value binds ciphertext to its public authority.
fn credential_cipher_context(
    id: &str,
    owner: &SponsorBillingOwner,
    provider: &str,
    provider_policy_key: &str,
    provider_credential_id_digest: &str,
    ownership_verified_at: &str,
    created_at: &str,
    provider_state_version: u64,
    supports_expiry: bool,
    supports_revocation: bool,
) -> Result<String, SponsorshipStoreError> {
    canonical_cipher_context(
        "cma-sponsorship-credential-cipher-authority-v1",
        &(
            id,
            owner,
            provider,
            provider_policy_key,
            provider_credential_id_digest,
            ownership_verified_at,
            created_at,
            provider_state_version,
            supports_expiry,
            supports_revocation,
        ),
    )
}

#[allow(clippy::too_many_arguments)] // Every value binds ciphertext to its activation authority.
fn activation_cipher_context(
    launch_id: &str,
    session_id: &str,
    policy_id: &str,
    policy_generation: u64,
    credential_generation: u64,
    launch_authority_digest: &str,
    acceptance_digest: &str,
    gate_fence: &SponsorshipGateFence,
    allocation: &SponsoredUsageSummary,
    request_fingerprint: &str,
    billing_request_id: &str,
    provider_request_id: &str,
    created_at: &str,
) -> Result<String, SponsorshipStoreError> {
    canonical_cipher_context(
        "cma-sponsorship-activation-cipher-authority-v1",
        &(
            launch_id,
            session_id,
            policy_id,
            policy_generation,
            credential_generation,
            launch_authority_digest,
            acceptance_digest,
            &gate_fence.configuration_digest,
            gate_fence.epoch,
            gate_fence.kill_switch_engaged,
            allocation,
            request_fingerprint,
            billing_request_id,
            provider_request_id,
            created_at,
        ),
    )
}

fn grant_cipher_context(grant: &SponsoredGrant) -> Result<String, SponsorshipStoreError> {
    canonical_cipher_context(
        "cma-sponsorship-grant-cipher-authority-v1",
        &(
            &grant.id.0,
            &grant.tenant_id,
            &grant.policy_id.0,
            grant.policy_generation,
            grant.credential_generation,
            &grant.trigger_revision_id,
            &grant.trigger_revision_digest,
            &grant.launch_id.0,
            &grant.session_id.0,
            &grant.contributor,
            &grant.terms_digest,
            &grant.acceptance,
            &grant.authority,
            &grant.allocation,
            &grant.created_at,
        ),
    )
}

fn model_dispatch_cipher_context(
    reservation: &SponsoredUsageReservation,
    credential_id: &SponsoredCredentialId,
    payload_schema_version: u32,
    request_digest: &str,
) -> Result<String, SponsorshipStoreError> {
    canonical_cipher_context(
        "cma-sponsorship-model-dispatch-cipher-authority-v1",
        &(
            DOCUMENT_SCHEMA_VERSION,
            payload_schema_version,
            &reservation.id.0,
            &reservation.request_fingerprint,
            request_digest,
            &reservation.grant_id.0,
            &credential_id.0,
            &reservation.price_quote.provider,
            &reservation.price_quote.model,
            &reservation.price_quote.resource,
            &reservation.price_quote.digest,
            &reservation.reserved,
            &reservation.created_at,
        ),
    )
}

fn model_response_cipher_context(
    reservation: &SponsoredUsageReservation,
    payload_schema_version: u32,
    request_digest: &str,
    response_digest: &str,
) -> Result<String, SponsorshipStoreError> {
    let actual = reservation.actual.as_ref().ok_or_else(|| {
        SponsorshipStoreError::Malformed(
            "settled model response context has no trusted accounting".into(),
        )
    })?;
    canonical_cipher_context(
        "cma-sponsorship-model-response-cipher-authority-v1",
        &(
            DOCUMENT_SCHEMA_VERSION,
            payload_schema_version,
            &reservation.id.0,
            &reservation.request_fingerprint,
            request_digest,
            response_digest,
            &reservation.grant_id.0,
            &reservation.price_quote.provider,
            &reservation.price_quote.model,
            &reservation.price_quote.resource,
            &reservation.price_quote.digest,
            &reservation.reserved,
            actual,
            &reservation.created_at,
        ),
    )
}

fn require_receipt_matches_grant(
    operational: &SponsorshipOperationalPseudonymizer,
    receipt: &ProviderGrantReceipt,
    grant: &SponsoredGrant,
) -> Result<(), SponsorshipStoreError> {
    let provider_grant_id_candidates = operational
        .pseudonymize(
            layer2_core::SponsorshipOperationalDomain::ProviderGrantAuthority,
            &[
                &grant.id.0,
                receipt
                    .provider_grant_id
                    .expose_for_provider_or_encryption(),
            ],
        )
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let revocation_handle_candidates = operational
        .pseudonymize(
            layer2_core::SponsorshipOperationalDomain::ProviderRevocationAuthority,
            &[
                &grant.id.0,
                receipt
                    .revocation_handle
                    .expose_for_provider_or_encryption(),
            ],
        )
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if !provider_grant_id_candidates
        .iter()
        .any(|candidate| candidate.as_str() == grant.authority.provider_grant_id_digest)
        || !revocation_handle_candidates
            .iter()
            .any(|candidate| candidate.as_str() == grant.authority.revocation_handle_digest)
        || grant.authority.provider_state_version != receipt.provider_state_version
        || grant.authority.expires_at != receipt.expires_at
        || grant.authority.issued_at != receipt.issued_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "encrypted provider receipt does not match the public grant authority".into(),
        ));
    }
    Ok(())
}

impl MongoSponsorshipStore {
    async fn complete_credential_mutation(
        &self,
        expected_generation: u64,
        provider_identity: Option<&SponsorshipOperationalPseudonymSet>,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
        kind: ProviderOperationKind,
        outcome: CompletedProviderOperationOutcome,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        require_successor_generation(expected_generation, record.generation, "credential")?;
        if let Some(provider_identity) = provider_identity {
            validate_operational_aliases(
                &self.operational,
                &provider_identity.encoded_candidates(),
                "provider credential rotation candidate",
            )?;
            if record.authority.provider_credential_id_digest
                != provider_identity.current().as_str()
            {
                return Err(SponsorshipStoreError::Malformed(
                    "rotated credential canonical provider identity is not current".into(),
                ));
            }
        }
        if !matches!(
            (kind, outcome),
            (
                ProviderOperationKind::RotateCredential,
                CompletedProviderOperationOutcome::Confirmed
                    | CompletedProviderOperationOutcome::Rejected
            ) | (
                ProviderOperationKind::RevokeCredential,
                CompletedProviderOperationOutcome::Confirmed
            )
        ) {
            return Err(SponsorshipStoreError::Conflict);
        }

        self.transaction(
            (
                self.clone(),
                expected_generation,
                provider_identity.cloned(),
                record.clone(),
                audit,
                kind,
                outcome,
            ),
            |session, (store, expected, provider_identity, candidate, audit, kind, outcome)| {
                async move {
                    let current = store
                        .credential_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if current.record.generation != *expected {
                        return if current.record == *candidate {
                            Ok(current.record)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
                    }
                    if !credential_completion_shape_is_legal(
                        &current.record,
                        candidate,
                        *kind,
                        *outcome,
                    ) {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let completion = credential_operation_completion(
                        &current.record,
                        candidate,
                        *kind,
                        *outcome,
                    )
                    .map_err(custom)?;
                    let ledger = ledger_doc(&candidate.id, &completion);
                    if let Some(stored) = store
                        .credential_operations
                        .find_one(doc! {
                            "_id.credential_id": &candidate.id.0,
                            "_id.operation_kind": operation_kind_code(*kind),
                            "_id.client_request_id_digest":
                                &completion.client_request_id_digest,
                        })
                        .session(&mut *session)
                        .await?
                    {
                        let stored = read_ledger(stored).map_err(custom)?;
                        if stored != completion {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                    }

                    let (current_secret, pending_secret) = match (*kind, *outcome) {
                        (
                            ProviderOperationKind::RotateCredential,
                            CompletedProviderOperationOutcome::Confirmed,
                        ) => {
                            if candidate.material_state
                                != SponsoredCredentialMaterialState::Retained
                            {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            let provider_identity = provider_identity.as_ref().ok_or_else(|| {
                                custom(SponsorshipStoreError::Malformed(
                                    "credential rotation completion omitted provider identity candidates"
                                        .into(),
                                ))
                            })?;
                            if store
                                .provider_credential_binding_in_transaction(
                                    session,
                                    &candidate.authority.provider,
                                    provider_identity,
                                )
                                .await?
                                .is_some_and(|document| document.id != candidate.id.0)
                            {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            let replacement = current.pending_secret.as_ref().ok_or_else(|| {
                                custom(SponsorshipStoreError::Malformed(
                                    "pending credential rotation material is missing".into(),
                                ))
                            })?;
                            (Some(replacement.expose()), None)
                        }
                        (
                            ProviderOperationKind::RotateCredential,
                            CompletedProviderOperationOutcome::Rejected,
                        ) => {
                            if candidate.material_state
                                != SponsoredCredentialMaterialState::Retained
                            {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            let predecessor = current.current_secret.as_ref().ok_or_else(|| {
                                custom(SponsorshipStoreError::Malformed(
                                    "current credential material is missing".into(),
                                ))
                            })?;
                            (Some(predecessor.expose()), None)
                        }
                        (
                            ProviderOperationKind::RevokeCredential,
                            CompletedProviderOperationOutcome::Confirmed,
                        ) => {
                            if candidate.state != SponsoredCredentialState::Revoked
                                || candidate.material_state
                                    != SponsoredCredentialMaterialState::Retired
                                || candidate.pending_operation.is_some()
                                || candidate.superseded_operation.is_some()
                            {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            if store
                                .reservations
                                .find_one(doc! {
                                    "credential_id": &candidate.id.0,
                                    "state": { "$in": ["reserved", "attempting", "ambiguous"] },
                                    "model_dispatch_payload": { "$exists": true },
                                })
                                .session(&mut *session)
                                .await?
                                .is_some()
                            {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            (None, None)
                        }
                        _ => return Err(custom(SponsorshipStoreError::Conflict)),
                    };
                    let provider_aliases = provider_identity
                        .as_ref()
                        .map(SponsorshipOperationalPseudonymSet::encoded_candidates)
                        .unwrap_or_else(|| current.provider_credential_id_aliases.clone());
                    let replacement = credential_doc(
                        &store.cipher,
                        candidate,
                        &provider_aliases,
                        current_secret,
                        pending_secret,
                        current.authority_read_revision,
                    )
                    .map_err(custom)?;
                    let result = store
                        .credentials
                        .replace_one(
                            doc! {
                                "_id": &candidate.id.0,
                                "generation": signed(*expected, "credential generation")
                                    .map_err(custom)?,
                                "authority_read_revision": current.authority_read_revision,
                            },
                            replacement,
                        )
                        .session(&mut *session)
                        .await?;
                    if result.modified_count != 1 {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    store
                        .credential_operations
                        .insert_one(ledger)
                        .session(&mut *session)
                        .await?;
                    store.audit_in_transaction(session, audit).await?;
                    Ok(candidate.clone())
                }
                .boxed()
            },
        )
        .await
    }

    async fn activation_in_transaction(
        &self,
        session: &mut ClientSession,
        launch_id: &LaunchId,
    ) -> mongodb::error::Result<Option<(GrantActivationIntent, Option<DateTime>)>> {
        self.activations
            .find_one(doc! { "_id": &launch_id.0 })
            .session(session)
            .await?
            .map(|document| {
                read_activation(&self.cipher, &self.operational, document).map_err(custom)
            })
            .transpose()
    }

    async fn grant_in_transaction(
        &self,
        session: &mut ClientSession,
        grant_id: &SponsoredGrantId,
    ) -> mongodb::error::Result<Option<(SponsoredGrant, GrantDoc)>> {
        self.grants
            .find_one(doc! { "_id": &grant_id.0 })
            .session(session)
            .await?
            .map(|document| {
                read_grant_record(&self.operational, &document)
                    .map(|grant| (grant, document))
                    .map_err(custom)
            })
            .transpose()
    }

    async fn grant_authority_in_transaction(
        &self,
        session: &mut ClientSession,
        grant_id: &SponsoredGrantId,
    ) -> mongodb::error::Result<Option<(SponsoredGrant, GrantDoc)>> {
        self.grants
            .find_one_and_update(
                doc! { "_id": &grant_id.0 },
                doc! { "$inc": { "authority_read_revision": 1_i64 } },
            )
            .return_document(ReturnDocument::After)
            .session(session)
            .await?
            .map(|document| {
                read_grant_record(&self.operational, &document)
                    .map(|grant| (grant, document))
                    .map_err(custom)
            })
            .transpose()
    }

    async fn reservation_in_transaction(
        &self,
        session: &mut ClientSession,
        reservation_id: &SponsoredUsageReservationId,
    ) -> mongodb::error::Result<
        Option<(
            SponsoredUsageReservation,
            SponsorshipPolicyId,
            DateTime,
            Vec<String>,
            ReservationDoc,
        )>,
    > {
        self.reservations
            .find_one(doc! { "_id": &reservation_id.0 })
            .session(session)
            .await?
            .map(|document| {
                let request_aliases = document.request_aliases.clone();
                read_reservation(&self.operational, document.clone())
                    .map(|(reservation, policy_id, observed_at)| {
                        (
                            reservation,
                            policy_id,
                            observed_at,
                            request_aliases,
                            document,
                        )
                    })
                    .map_err(custom)
            })
            .transpose()
    }

    async fn reservation_for_request_in_transaction(
        &self,
        session: &mut ClientSession,
        candidates: &SponsorshipOperationalPseudonymSet,
    ) -> mongodb::error::Result<
        Option<(
            SponsoredUsageReservation,
            SponsorshipPolicyId,
            DateTime,
            Vec<String>,
        )>,
    > {
        let aliases = candidates.encoded_candidates();
        validate_operational_aliases(&self.operational, &aliases, "usage request candidate")
            .map_err(custom)?;
        let mut cursor = self
            .reservations
            .find(doc! { "request_aliases": { "$in": aliases } })
            .limit((MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS + 2) as i64)
            .session(&mut *session)
            .await?;
        let matches = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if matches.len() > 1 {
            return Err(custom(SponsorshipStoreError::Malformed(
                "usage request candidates resolve multiple durable reservations".into(),
            )));
        }
        matches
            .into_iter()
            .next()
            .map(|document| {
                let request_aliases = document.request_aliases.clone();
                read_reservation(&self.operational, document)
                    .map(|(reservation, policy_id, observed_at)| {
                        (reservation, policy_id, observed_at, request_aliases)
                    })
                    .map_err(custom)
            })
            .transpose()
    }

    async fn acceptance_in_transaction(
        &self,
        session: &mut ClientSession,
        launch_id: &LaunchId,
    ) -> mongodb::error::Result<Option<SponsoredGrantAcceptance>> {
        self.acceptances
            .find_one_and_update(
                doc! { "_id": &launch_id.0 },
                doc! { "$inc": { "authority_read_revision": 1_i64 } },
            )
            .return_document(ReturnDocument::After)
            .session(session)
            .await?
            .map(|document| read_acceptance(&self.operational, document).map_err(custom))
            .transpose()
    }

    async fn replace_policy_usage_in_transaction(
        &self,
        session: &mut ClientSession,
        policy: &SponsorshipPolicy,
        usage: &SponsorshipPolicyUsage,
    ) -> mongodb::error::Result<()> {
        let replacement = policy_doc(policy, usage).map_err(custom)?;
        let result = self
            .policies
            .replace_one(
                doc! {
                    "_id": &policy.id.0,
                    "policy.generation": signed(policy.generation, "policy generation")
                        .map_err(custom)?,
                },
                replacement,
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(custom(SponsorshipStoreError::StaleGeneration));
        }
        Ok(())
    }

    async fn advance_policy_rate_fence_in_transaction(
        &self,
        session: &mut ClientSession,
        policy_id: &SponsorshipPolicyId,
    ) -> mongodb::error::Result<()> {
        let current = self
            .policy_counters
            .find_one(doc! { "_id": &policy_id.0 })
            .session(&mut *session)
            .await?
            .ok_or_else(|| {
                custom(SponsorshipStoreError::Malformed(
                    "sponsorship policy rate counter is missing".into(),
                ))
            })?;
        validate_document_version(
            current.document_schema_version,
            "sponsorship policy rate counter",
        )
        .map_err(custom)?;
        if current.policy_id != policy_id.0 || current.rate_revision < 0 {
            return Err(custom(SponsorshipStoreError::Malformed(
                "sponsorship policy rate counter is invalid".into(),
            )));
        }
        let next_revision = current.rate_revision.checked_add(1).ok_or_else(|| {
            custom(SponsorshipStoreError::Malformed(
                "sponsorship policy rate counter exhausted".into(),
            ))
        })?;
        let result = self
            .policy_counters
            .replace_one(
                doc! {
                    "_id": &policy_id.0,
                    "rate_revision": current.rate_revision,
                },
                PolicyCounterDoc {
                    policy_id: policy_id.0.clone(),
                    document_schema_version: DOCUMENT_SCHEMA_VERSION,
                    rate_revision: next_revision,
                },
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(custom(SponsorshipStoreError::Conflict));
        }
        Ok(())
    }

    async fn advance_sponsor_admission_fence_in_transaction(
        &self,
        session: &mut ClientSession,
        sponsor_key: &str,
        fence: SponsorAdmissionFence,
    ) -> mongodb::error::Result<()> {
        let current = self
            .sponsor_counters
            .find_one(doc! { "_id": sponsor_key })
            .session(&mut *session)
            .await?
            .ok_or_else(|| {
                custom(SponsorshipStoreError::Malformed(
                    "sponsorship sponsor admission counter is missing".into(),
                ))
            })?;
        validate_sponsor_counter(&current, sponsor_key).map_err(custom)?;
        let mut candidate = current.clone();
        let (field, expected) = match fence {
            SponsorAdmissionFence::Policy => {
                candidate.policy_revision =
                    candidate.policy_revision.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "sponsorship sponsor policy counter exhausted".into(),
                        ))
                    })?;
                ("policy_revision", current.policy_revision)
            }
            SponsorAdmissionFence::Launch => {
                candidate.launch_revision =
                    candidate.launch_revision.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "sponsorship sponsor Launch counter exhausted".into(),
                        ))
                    })?;
                ("launch_revision", current.launch_revision)
            }
        };
        let result = self
            .sponsor_counters
            .replace_one(doc! { "_id": sponsor_key, field: expected }, candidate)
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(custom(SponsorshipStoreError::Conflict));
        }
        Ok(())
    }

    async fn admit_sponsor_launch_in_transaction(
        &self,
        session: &mut ClientSession,
        launch_id: &LaunchId,
        sponsor_key: &str,
        gate_fence: &SponsorshipGateFence,
        now: DateTime,
    ) -> mongodb::error::Result<()> {
        self.advance_sponsor_admission_fence_in_transaction(
            session,
            sponsor_key,
            SponsorAdmissionFence::Launch,
        )
        .await?;
        let window_millis = i64::try_from(gate_fence.runtime_limits.launch_rate_window_seconds)
            .ok()
            .and_then(|seconds| seconds.checked_mul(1_000))
            .ok_or_else(|| {
                custom(SponsorshipStoreError::Malformed(
                    "sponsorship sponsor Launch window is out of range".into(),
                ))
            })?;
        let window_start = DateTime::from_millis(
            now.timestamp_millis()
                .checked_sub(window_millis)
                .ok_or_else(|| {
                    custom(SponsorshipStoreError::Malformed(
                        "sponsorship sponsor Launch window underflowed".into(),
                    ))
                })?,
        );
        let recent = self
            .sponsor_launch_admissions
            .count_documents(doc! {
                "sponsor_key": sponsor_key,
                "admitted_at_database": { "$gt": window_start },
            })
            .session(&mut *session)
            .await?;
        if recent
            >= u64::from(
                gate_fence
                    .runtime_limits
                    .maximum_launches_per_sponsor_per_window,
            )
        {
            return Err(custom(SponsorshipStoreError::SponsorLaunchRateExceeded));
        }
        self.sponsor_launch_admissions
            .insert_one(SponsorLaunchAdmissionDoc {
                launch_id: launch_id.0.clone(),
                document_schema_version: DOCUMENT_SCHEMA_VERSION,
                sponsor_key: sponsor_key.to_string(),
                admitted_at_database: now,
            })
            .session(session)
            .await?;
        Ok(())
    }

    async fn replace_activation_in_transaction(
        &self,
        session: &mut ClientSession,
        expected_revision: u64,
        intent: &GrantActivationIntent,
        lease_expiry_database: Option<DateTime>,
    ) -> mongodb::error::Result<()> {
        let replacement =
            activation_doc(&self.cipher, intent, lease_expiry_database).map_err(custom)?;
        let result = self
            .activations
            .replace_one(
                doc! {
                    "_id": &intent.launch_id.0,
                    "revision": signed(expected_revision, "activation revision")
                        .map_err(custom)?,
                },
                replacement,
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(custom(SponsorshipStoreError::StaleGeneration));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)] // The CAS commits one exact encrypted grant tuple.
    async fn replace_grant_in_transaction(
        &self,
        session: &mut ClientSession,
        expected_generation: u64,
        grant: &SponsoredGrant,
        sponsor_subject: &str,
        unresolved_usage: SponsoredUsageSummary,
        encrypted_receipt: ProviderReceiptDoc,
        encryption_key_id: String,
    ) -> mongodb::error::Result<()> {
        let replacement = grant_doc_with_encrypted_receipt(
            grant,
            sponsor_subject,
            unresolved_usage,
            encrypted_receipt,
            encryption_key_id,
        )
        .map_err(custom)?;
        let result = self
            .grants
            .replace_one(
                doc! {
                    "_id": &grant.id.0,
                    "generation": signed(expected_generation, "grant generation")
                        .map_err(custom)?,
                },
                replacement,
            )
            .session(session)
            .await?;
        if result.matched_count != 1 {
            return Err(custom(SponsorshipStoreError::StaleGeneration));
        }
        Ok(())
    }

    async fn current_activation_dependencies_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
        now: &str,
    ) -> mongodb::error::Result<(
        SponsoredLaunchAuthority,
        SponsoredGrantAcceptance,
        SponsorshipPolicy,
        SponsoredCredentialRecord,
    )> {
        self.require_gate_in_transaction(session, &intent.gate_fence)
            .await?;
        let launch = self
            .launch_in_transaction(session, &intent.launch_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
        let policy_id = launch_sponsorship_policy_id(&launch).map_err(custom)?;
        let stored_acceptance = self
            .acceptance_in_transaction(session, &intent.launch_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::AcceptanceChanged))?;
        let (policy, usage) = self
            .policy_in_transaction(session, &policy_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
        let credential = self
            .credential_record_in_transaction(session, &policy.credential_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
        let current_launch = SponsoredLaunchAuthority::from_launch(&launch, &policy)
            .map_err(|_| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
        if current_launch.state != SponsoredLaunchState::Starting
            || current_launch.session_id.as_ref() != Some(&intent.session_id)
            || current_launch.digest != intent.launch_authority_digest
        {
            return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
        }
        if stored_acceptance.digest != intent.acceptance_digest
            || stored_acceptance.launch_id != current_launch.launch_id
            || stored_acceptance.contributor != current_launch.contributor
            || stored_acceptance.revision_id != current_launch.revision_id
            || stored_acceptance.revision_digest != current_launch.revision_digest
            || stored_acceptance.policy_id != current_launch.policy_id
            || stored_acceptance.launch_snapshot_digest != current_launch.launch_snapshot_digest
        {
            return Err(custom(SponsorshipStoreError::AcceptanceChanged));
        }
        let terms = SponsoredGrantTerms::from_policy(&policy, &usage)
            .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
        if intent
            .gate_fence
            .runtime_limits
            .require_policy_at(&policy, now)
            .is_err()
            || policy.provider_policy.provider_policy_key
                != intent.gate_fence.trusted_provider_policy_key
            || !policy.is_available_at(now)
            || policy.id != intent.policy_id
            || policy.generation != intent.policy_generation
            || stored_acceptance.policy_generation != policy.generation
            || stored_acceptance.terms_digest != terms.digest
            || credential.state != SponsoredCredentialState::Active
            || credential.material_state != SponsoredCredentialMaterialState::Retained
            || credential.generation != intent.credential_generation
            || credential.owner != policy.owner
            || credential.authority.provider != policy.provider_policy.provider
            || credential.authority.provider_policy_key
                != policy.provider_policy.provider_policy_key
            || intent.allocation.quota != policy.per_launch_quota
            || intent.allocation.spend_micros != policy.per_launch_budget_micros
        {
            return Err(custom(SponsorshipStoreError::GrantUnavailable));
        }
        Ok((current_launch, stored_acceptance, policy, credential))
    }

    async fn require_activation_dependencies_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
        launch: &SponsoredLaunchAuthority,
        acceptance: &SponsoredGrantAcceptance,
        now: &str,
    ) -> mongodb::error::Result<(SponsorshipPolicy, SponsoredCredentialRecord)> {
        let (current_launch, stored_acceptance, policy, credential) = self
            .current_activation_dependencies_in_transaction(session, intent, now)
            .await?;
        if current_launch != *launch {
            return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
        }
        if stored_acceptance != *acceptance {
            return Err(custom(SponsorshipStoreError::AcceptanceChanged));
        }
        Ok((policy, credential))
    }

    async fn activation_dependencies_are_current_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
        now: DateTime,
    ) -> mongodb::error::Result<bool> {
        let now_string = database_time_string(now);
        if let Err(error) = self
            .current_activation_dependencies_in_transaction(session, intent, &now_string)
            .await
        {
            if error
                .get_custom::<SponsorshipStoreError>()
                .is_some_and(|error| {
                    matches!(
                        error,
                        SponsorshipStoreError::GateChanged
                            | SponsorshipStoreError::LaunchAuthorityChanged
                            | SponsorshipStoreError::AcceptanceChanged
                            | SponsorshipStoreError::GrantUnavailable
                    )
                })
            {
                return Ok(false);
            }
            return Err(error);
        }
        if let Some(receipt) = &intent.provider_receipt {
            let now = database_time_seconds(now).map_err(custom)?;
            let issued = timestamp_seconds(&receipt.issued_at, "provider receipt issue time")
                .map_err(custom)?;
            let expires = timestamp_seconds(&receipt.expires_at, "provider receipt expiry")
                .map_err(custom)?;
            if issued > now || expires <= now {
                return Ok(false);
            }
        }
        Ok(true)
    }

    async fn reserve_activation_allocation_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
    ) -> mongodb::error::Result<()> {
        let (policy, mut usage) = self
            .policy_in_transaction(session, &intent.policy_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
        if usage.active_launches >= policy.max_concurrent_launches {
            return Err(custom(SponsorshipStoreError::ConcurrentLaunchLimit));
        }
        let exposure = summary_add(&usage.consumed, &usage.reserved)
            .and_then(|value| summary_add(&value, &intent.allocation))
            .map_err(custom)?;
        if !exposure.quota.fits_within(policy.aggregate_quota) {
            return Err(custom(SponsorshipStoreError::AggregateQuotaExceeded));
        }
        if exposure.spend_micros > policy.aggregate_budget_micros {
            return Err(custom(SponsorshipStoreError::AggregateBudgetExceeded));
        }
        usage.reserved = summary_add(&usage.reserved, &intent.allocation).map_err(custom)?;
        usage.active_launches = usage
            .active_launches
            .checked_add(1)
            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
        self.replace_policy_usage_in_transaction(session, &policy, &usage)
            .await
    }

    async fn release_activation_allocation_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
    ) -> mongodb::error::Result<()> {
        let (policy, mut usage) = self
            .policy_in_transaction(session, &intent.policy_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
        self.release_activation_allocation_for_policy_in_transaction(
            session, intent, &policy, &mut usage,
        )
        .await
    }

    async fn release_activation_allocation_for_policy_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
        policy: &SponsorshipPolicy,
        usage: &mut SponsorshipPolicyUsage,
    ) -> mongodb::error::Result<()> {
        if intent.policy_id != policy.id {
            return Err(custom(SponsorshipStoreError::Malformed(
                "activation allocation references a different sponsorship policy".into(),
            )));
        }
        usage.reserved = summary_sub(&usage.reserved, &intent.allocation).map_err(custom)?;
        usage.active_launches = usage
            .active_launches
            .checked_sub(1)
            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
        self.replace_policy_usage_in_transaction(session, policy, usage)
            .await
    }

    async fn release_terminal_grant_capacity_in_transaction(
        &self,
        session: &mut ClientSession,
        grant: &SponsoredGrant,
        unresolved_usage: &SponsoredUsageSummary,
    ) -> mongodb::error::Result<()> {
        let unused = summary_sub(&grant.allocation, &grant.usage).map_err(custom)?;
        let releasable = summary_sub(&unused, unresolved_usage).map_err(custom)?;
        let (policy, mut usage) = self
            .policy_in_transaction(session, &grant.policy_id)
            .await?
            .ok_or_else(|| {
                custom(SponsorshipStoreError::Malformed(
                    "stored grant references a missing sponsorship policy".into(),
                ))
            })?;
        require_grant_policy_bindings(grant, &policy).map_err(custom)?;
        usage.reserved = summary_sub(&usage.reserved, &releasable).map_err(custom)?;
        usage.active_launches = usage
            .active_launches
            .checked_sub(1)
            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
        self.replace_policy_usage_in_transaction(session, &policy, &usage)
            .await
    }

    async fn audit_in_transaction(
        &self,
        session: &mut ClientSession,
        event: &SponsoredAuditEvent,
    ) -> mongodb::error::Result<()> {
        let document = audit_doc(event).map_err(custom)?;
        self.audits.insert_one(document).session(session).await?;
        Ok(())
    }

    async fn credential_in_transaction(
        &self,
        session: &mut ClientSession,
        id: &SponsoredCredentialId,
    ) -> mongodb::error::Result<Option<DecodedCredential>> {
        self.credentials
            .find_one_and_update(
                doc! { "_id": &id.0 },
                doc! { "$inc": { "authority_read_revision": 1_i64 } },
            )
            .return_document(ReturnDocument::After)
            .session(session)
            .await?
            .map(|document| {
                read_credential(&self.cipher, &self.operational, document).map_err(custom)
            })
            .transpose()
    }

    async fn credential_registration_binding_in_transaction(
        &self,
        session: &mut ClientSession,
        candidates: &SponsorshipOperationalPseudonymSet,
    ) -> mongodb::error::Result<Option<CredentialRegistrationDoc>> {
        let mut cursor = self
            .credential_registrations
            .find(doc! {
                "request_aliases": { "$in": candidates.encoded_candidates() }
            })
            .limit((MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS + 2) as i64)
            .session(&mut *session)
            .await?;
        let matches = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if matches.len() > 1 {
            return Err(custom(SponsorshipStoreError::Malformed(
                "credential registration candidates resolve multiple durable bindings".into(),
            )));
        }
        let binding = matches.into_iter().next();
        if let Some(binding) = &binding {
            validate_credential_registration_doc(&self.operational, binding).map_err(custom)?;
        }
        Ok(binding)
    }

    async fn policy_creation_binding_in_transaction(
        &self,
        session: &mut ClientSession,
        candidates: &SponsorshipOperationalPseudonymSet,
    ) -> mongodb::error::Result<Option<PolicyCreationDoc>> {
        let mut cursor = self
            .policy_creations
            .find(doc! {
                "request_aliases": { "$in": candidates.encoded_candidates() }
            })
            .limit((MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS + 2) as i64)
            .session(&mut *session)
            .await?;
        let matches = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if matches.len() > 1 {
            return Err(custom(SponsorshipStoreError::Malformed(
                "policy creation candidates resolve multiple durable bindings".into(),
            )));
        }
        let binding = matches.into_iter().next();
        if let Some(binding) = &binding {
            validate_policy_creation_doc(&self.operational, binding).map_err(custom)?;
        }
        Ok(binding)
    }

    async fn provider_credential_binding_in_transaction(
        &self,
        session: &mut ClientSession,
        provider: &str,
        candidates: &SponsorshipOperationalPseudonymSet,
    ) -> mongodb::error::Result<Option<CredentialDoc>> {
        let mut cursor = self
            .credentials
            .find(doc! {
                "provider": provider,
                "provider_credential_id_aliases": { "$in": candidates.encoded_candidates() },
            })
            .limit((MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS + 2) as i64)
            .session(&mut *session)
            .await?;
        let matches = cursor.stream(&mut *session).try_collect::<Vec<_>>().await?;
        if matches.len() > 1 {
            return Err(custom(SponsorshipStoreError::Malformed(
                "provider credential candidates resolve multiple durable lineages".into(),
            )));
        }
        let binding = matches.into_iter().next();
        if let Some(binding) = &binding {
            validate_credential_provider_aliases(&self.operational, binding).map_err(custom)?;
        }
        Ok(binding)
    }

    async fn credential_record_in_transaction(
        &self,
        session: &mut ClientSession,
        id: &SponsoredCredentialId,
    ) -> mongodb::error::Result<Option<SponsoredCredentialRecord>> {
        self.credentials
            .find_one_and_update(
                doc! { "_id": &id.0 },
                doc! { "$inc": { "authority_read_revision": 1_i64 } },
            )
            .return_document(ReturnDocument::After)
            .session(session)
            .await?
            .map(|document| {
                read_credential_record(&self.cipher, &self.operational, &document).map_err(custom)
            })
            .transpose()
    }

    async fn policy_in_transaction(
        &self,
        session: &mut ClientSession,
        id: &SponsorshipPolicyId,
    ) -> mongodb::error::Result<Option<(SponsorshipPolicy, SponsorshipPolicyUsage)>> {
        self.policies
            .find_one_and_update(
                doc! { "_id": &id.0 },
                doc! { "$inc": { "authority_read_revision": 1_i64 } },
            )
            .return_document(ReturnDocument::After)
            .session(session)
            .await?
            .map(|document| read_policy(document).map_err(custom))
            .transpose()
    }

    async fn require_gate_in_transaction(
        &self,
        session: &mut ClientSession,
        expected: &SponsorshipGateFence,
    ) -> mongodb::error::Result<()> {
        let document = self
            .gate
            .find_one_and_update(
                doc! {
                    "_id": GATE_ID,
                    "configuration_digest": &expected.configuration_digest,
                    "epoch": signed(expected.epoch, "sponsorship gate epoch").map_err(custom)?,
                    "kill_switch_engaged": false,
                },
                doc! { "$inc": { "authority_read_revision": 1_i64 } },
            )
            .return_document(ReturnDocument::After)
            .session(session)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GateChanged))?;
        let current = read_gate(document).map_err(custom)?;
        if &current != expected {
            return Err(custom(SponsorshipStoreError::GateChanged));
        }
        Ok(())
    }

    async fn launch_in_transaction(
        &self,
        session: &mut ClientSession,
        id: &LaunchId,
    ) -> mongodb::error::Result<Option<Launch>> {
        let document = self
            .trigger_launches
            .find_one_and_update(
                doc! { "_id": &id.0 },
                doc! { "$inc": { "sponsorship_read_revision": 1_i64 } },
            )
            .return_document(ReturnDocument::After)
            .session(&mut *session)
            .await?;
        let Some(document) = document else {
            return Ok(None);
        };
        read_launch(document)
            .map_err(map_trigger_read_error)
            .map(Some)
            .map_err(custom)
    }

    async fn resume_kill_switch_drain(&self) -> Result<bool, SponsorshipStoreError> {
        let document = self
            .gate
            .find_one(doc! { "_id": GATE_ID })
            .await
            .map_err(|error| stored_read_failure("read kill-switch drain state", error))?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed("the sponsorship gate is missing".into())
            })?;
        let fence = read_gate(document.clone())?;
        if !fence.kill_switch_engaged || document.kill_switch_drain_complete {
            return Ok(true);
        }
        let engagement_id = document.kill_switch_engagement_id.ok_or_else(|| {
            SponsorshipStoreError::Malformed("kill-switch engagement identity is missing".into())
        })?;
        let audit = self
            .audits
            .find_one(doc! { "_id": &engagement_id })
            .await
            .map_err(|error| stored_read_failure("read kill-switch engagement audit", error))?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "unfinished kill-switch drain has no engagement audit".into(),
                )
            })?;
        let audit = read_audit(audit)?;
        self.drain_kill_switch_engagement(&engagement_id, &audit)
            .await
    }

    async fn drain_kill_switch_engagement(
        &self,
        engagement_id: &str,
        engagement_audit: &SponsoredAuditEvent,
    ) -> Result<bool, SponsorshipStoreError> {
        let mut cursor = self
            .activations
            .find(doc! {
                "state": { "$in": ["awaiting_billing", "awaiting_provider"] }
            })
            .sort(doc! { "created_at": 1, "_id": 1 })
            .limit(MAX_KILL_SWITCH_INVALIDATIONS)
            .await
            .map_err(|error| unavailable("discover kill-switch drain work", error))?;
        let mut activation_ids = Vec::new();
        while let Some(document) = cursor
            .try_next()
            .await
            .map_err(|error| unavailable("read kill-switch drain work", error))?
        {
            let (intent, _) = read_activation(&self.cipher, &self.operational, document)?;
            activation_ids.push((intent.launch_id, intent.policy_id));
        }
        for (launch_id, policy_id) in activation_ids {
            self.drain_one_kill_switch_activation(
                engagement_id,
                engagement_audit,
                &launch_id,
                &policy_id,
            )
            .await?;
        }
        let reservation_ids = self
            .reservations
            .clone_with_type::<ReservationIdDoc>()
            .find(doc! { "state": "reserved" })
            .sort(doc! { "created_at": 1, "_id": 1 })
            .projection(doc! { "_id": 1 })
            .limit(MAX_KILL_SWITCH_INVALIDATIONS)
            .await
            .map_err(|error| unavailable("discover reserved kill-switch drain work", error))?
            .map_ok(|document| document.reservation_id)
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read reserved kill-switch drain work", error))?;
        for reservation_id in reservation_ids {
            let reservation_id = SponsoredUsageReservationId::parse(reservation_id)
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
            self.drain_one_kill_switch_reservation(
                engagement_id,
                engagement_audit,
                &reservation_id,
            )
            .await?;
        }
        self.finish_kill_switch_drain(engagement_id).await
    }

    async fn drain_one_kill_switch_activation(
        &self,
        engagement_id: &str,
        engagement_audit: &SponsoredAuditEvent,
        launch_id: &LaunchId,
        policy_id: &SponsorshipPolicyId,
    ) -> Result<bool, SponsorshipStoreError> {
        self.transaction(
            (
                self.clone(),
                engagement_id.to_string(),
                engagement_audit.clone(),
                launch_id.clone(),
                policy_id.clone(),
            ),
            |session, (store, engagement_id, engagement_audit, launch_id, policy_id)| {
                async move {
                    let gate = store
                        .gate
                        .find_one_and_update(
                            doc! {
                                "_id": GATE_ID,
                                "kill_switch_engaged": true,
                                "kill_switch_engagement_id": &*engagement_id,
                                "kill_switch_drain_complete": false,
                            },
                            doc! { "$inc": { "authority_read_revision": 1_i64 } },
                        )
                        .return_document(ReturnDocument::After)
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::GateChanged))?;
                    read_gate(gate).map_err(custom)?;
                    let (policy, mut usage) = store
                        .policy_in_transaction(session, policy_id)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "drainable activation references a missing policy".into(),
                            ))
                        })?;
                    let Some((mut intent, _)) =
                        store.activation_in_transaction(session, launch_id).await?
                    else {
                        return Ok(false);
                    };
                    if !matches!(
                        intent.state,
                        GrantActivationState::AwaitingBilling
                            | GrantActivationState::AwaitingProvider
                    ) {
                        return Ok(false);
                    }
                    if intent.capacity_held {
                        store
                            .release_activation_allocation_for_policy_in_transaction(
                                session, &intent, &policy, &mut usage,
                            )
                            .await?;
                    }
                    let expected_revision = intent.revision;
                    intent.state = GrantActivationState::Denied;
                    intent.denial = Some(GrantActivationDenial::AuthorityInvalidated);
                    intent.capacity_held = false;
                    intent.lease = None;
                    intent.revision = intent.revision.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "activation revision exhausted".into(),
                        ))
                    })?;
                    intent.updated_at =
                        later_timestamp(&intent.updated_at, &engagement_audit.occurred_at)
                            .map_err(custom)?;
                    intent.validate().map_err(|error| {
                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                    })?;
                    let event = SponsoredAuditEvent {
                        event_id: deterministic_uuid(
                            "cma-sponsorship-kill-switch-drain-audit-v1",
                            &[engagement_id, &intent.launch_id.0],
                        ),
                        action: SponsoredAuditAction::GrantDenied,
                        actor_digest: engagement_audit.actor_digest.clone(),
                        policy_id: Some(intent.policy_id.clone()),
                        grant_id: None,
                        stable_request_id_digest: None,
                        outcome_code: "kill_switch_invalidated".into(),
                        occurred_at: intent.updated_at.clone(),
                    };
                    store
                        .replace_activation_in_transaction(
                            session,
                            expected_revision,
                            &intent,
                            None,
                        )
                        .await?;
                    store.audit_in_transaction(session, &event).await?;
                    Ok(true)
                }
                .boxed()
            },
        )
        .await
    }

    async fn drain_one_kill_switch_reservation(
        &self,
        engagement_id: &str,
        engagement_audit: &SponsoredAuditEvent,
        reservation_id: &SponsoredUsageReservationId,
    ) -> Result<bool, SponsorshipStoreError> {
        self.transaction(
            (
                self.clone(),
                engagement_id.to_string(),
                engagement_audit.clone(),
                reservation_id.clone(),
            ),
            |session, (store, engagement_id, engagement_audit, reservation_id)| {
                async move {
                    let gate = store
                        .gate
                        .find_one_and_update(
                            doc! {
                                "_id": GATE_ID,
                                "kill_switch_engaged": true,
                                "kill_switch_engagement_id": &*engagement_id,
                                "kill_switch_drain_complete": false,
                            },
                            doc! { "$inc": { "authority_read_revision": 1_i64 } },
                        )
                        .return_document(ReturnDocument::After)
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::GateChanged))?;
                    read_gate(gate).map_err(custom)?;
                    let Some((current, policy_id, _, _, document)) = store
                        .reservation_in_transaction(session, reservation_id)
                        .await?
                    else {
                        return Ok(false);
                    };
                    if current.state != SponsoredUsageState::Reserved {
                        return Ok(false);
                    }
                    let (grant, grant_document) = store
                        .grant_in_transaction(session, &current.grant_id)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "reserved usage references a missing grant".into(),
                            ))
                        })?;
                    if grant.policy_id != policy_id {
                        return Err(custom(SponsorshipStoreError::Malformed(
                            "reserved usage policy index does not match its grant".into(),
                        )));
                    }
                    let unresolved_usage =
                        summary_sub(&grant_document.unresolved_usage, &current.reserved)
                            .map_err(custom)?;
                    if grant_capacity_is_released(grant.state) {
                        let (policy, mut usage) = store
                            .policy_in_transaction(session, &policy_id)
                            .await?
                            .ok_or_else(|| {
                                custom(SponsorshipStoreError::Malformed(
                                    "reserved usage references a missing policy".into(),
                                ))
                            })?;
                        require_grant_policy_bindings(&grant, &policy).map_err(custom)?;
                        usage.reserved =
                            summary_sub(&usage.reserved, &current.reserved).map_err(custom)?;
                        store
                            .replace_policy_usage_in_transaction(session, &policy, &usage)
                            .await?;
                    }
                    store
                        .replace_grant_in_transaction(
                            session,
                            grant.generation,
                            &grant,
                            &grant_document.sponsor_subject,
                            unresolved_usage,
                            grant_document.provider_receipt,
                            grant_document.encryption_key_id,
                        )
                        .await?;

                    let now = database_time_string(store.database_now(session).await?);
                    let mut released = current.clone();
                    released.state = SponsoredUsageState::Released;
                    released.dispatch_lease = None;
                    released.actual = None;
                    released.updated_at =
                        later_timestamp(&current.updated_at, &now).map_err(custom)?;
                    released.validate().map_err(|error| {
                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                    })?;
                    let replacement =
                        replace_reservation_projection(document, &released).map_err(custom)?;
                    let result = store
                        .reservations
                        .replace_one(
                            doc! {
                                "_id": &released.id.0,
                                "state": "reserved",
                                "updated_at": &current.updated_at,
                                "reservation.dispatch_revision": 0_i64,
                                "reservation.dispatch_lease": { "$exists": false },
                            },
                            replacement,
                        )
                        .session(&mut *session)
                        .await?;
                    if result.matched_count != 1 {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    let event = SponsoredAuditEvent {
                        event_id: deterministic_uuid(
                            "cma-sponsorship-kill-switch-usage-drain-audit-v1",
                            &[engagement_id, &released.id.0],
                        ),
                        action: SponsoredAuditAction::UsageReconciled,
                        actor_digest: engagement_audit.actor_digest.clone(),
                        policy_id: Some(policy_id),
                        grant_id: Some(grant.id),
                        stable_request_id_digest: None,
                        outcome_code: "kill_switch_released_reserved".into(),
                        occurred_at: released.updated_at,
                    };
                    store.audit_in_transaction(session, &event).await?;
                    Ok(true)
                }
                .boxed()
            },
        )
        .await
    }

    async fn finish_kill_switch_drain(
        &self,
        engagement_id: &str,
    ) -> Result<bool, SponsorshipStoreError> {
        self.transaction(
            (self.clone(), engagement_id.to_string()),
            |session, (store, engagement_id)| {
                async move {
                    let gate = store
                        .gate
                        .find_one_and_update(
                            doc! {
                                "_id": GATE_ID,
                                "kill_switch_engaged": true,
                                "kill_switch_engagement_id": &*engagement_id,
                            },
                            doc! { "$inc": { "authority_read_revision": 1_i64 } },
                        )
                        .return_document(ReturnDocument::After)
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::GateChanged))?;
                    let fence = read_gate(gate.clone()).map_err(custom)?;
                    if gate.kill_switch_drain_complete {
                        return Ok(true);
                    }
                    if !fence.kill_switch_engaged {
                        return Err(custom(SponsorshipStoreError::GateChanged));
                    }
                    if store
                        .activations
                        .find_one(doc! {
                            "state": { "$in": ["awaiting_billing", "awaiting_provider"] }
                        })
                        .session(&mut *session)
                        .await?
                        .is_some()
                    {
                        return Ok(false);
                    }
                    if store
                        .reservations
                        .find_one(doc! { "state": "reserved" })
                        .session(&mut *session)
                        .await?
                        .is_some()
                    {
                        return Ok(false);
                    }
                    let result = store
                        .gate
                        .update_one(
                            doc! {
                                "_id": GATE_ID,
                                "kill_switch_engaged": true,
                                "kill_switch_engagement_id": &*engagement_id,
                                "kill_switch_drain_complete": false,
                            },
                            doc! { "$set": { "kill_switch_drain_complete": true } },
                        )
                        .session(&mut *session)
                        .await?;
                    if result.modified_count != 1 {
                        return Err(custom(SponsorshipStoreError::GateChanged));
                    }
                    Ok(true)
                }
                .boxed()
            },
        )
        .await
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryCursorWire {
    version: u8,
    domain: String,
    updated_at: String,
    id: String,
}

fn validate_page_cursor(cursor: &PageCursor) -> Result<(), SponsorshipStoreError> {
    if layer2_core::seconds_from_rfc3339(&cursor.created_at).is_none()
        || cursor.id.is_empty()
        || cursor.id.len() > 512
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship page cursor is not usable".into(),
        ));
    }
    Ok(())
}

fn validate_subject_query(subject: &SubjectId) -> Result<(), SponsorshipStoreError> {
    if subject.0.is_empty() || subject.0.len() > 255 {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship owner or contributor subject is not usable".into(),
        ));
    }
    Ok(())
}

fn newest_page_filter(
    base: Document,
    page: &PageRequest,
) -> Result<Document, SponsorshipStoreError> {
    let Some(cursor) = page.cursor() else {
        return Ok(base);
    };
    validate_page_cursor(cursor)?;
    Ok(doc! {
        "$and": [
            base,
            {
                "$or": [
                    { "created_at": { "$lt": &cursor.created_at } },
                    {
                        "created_at": &cursor.created_at,
                        "_id": { "$lt": &cursor.id },
                    },
                ],
            },
        ],
    })
}

fn page_query_limit(page: &PageRequest) -> i64 {
    i64::from(page.limit()) + 1
}

fn finish_newest_page<T>(
    mut items: Vec<T>,
    page: &PageRequest,
    key: impl Fn(&T) -> (&str, &str),
) -> Page<T> {
    let limit = page.limit() as usize;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next = has_more.then(|| {
        let (created_at, id) = key(items
            .last()
            .expect("a full sponsorship page has a final row"));
        PageCursor::new(created_at, id)
    });
    Page { items, next }
}

fn decode_recovery_cursor(
    page: &SponsorshipRecoveryPageRequest,
    domain: &str,
) -> Result<Option<(String, String)>, SponsorshipStoreError> {
    let Some(cursor) = page.cursor() else {
        return Ok(None);
    };
    if cursor.is_empty() || cursor.len() > MAX_SPONSORSHIP_RECOVERY_CURSOR_BYTES {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship recovery cursor is not usable".into(),
        ));
    }
    let decoded = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| {
        SponsorshipStoreError::Malformed("sponsorship recovery cursor is not usable".into())
    })?;
    if decoded.len() > MAX_SPONSORSHIP_RECOVERY_CURSOR_BYTES {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship recovery cursor is not usable".into(),
        ));
    }
    let cursor: RecoveryCursorWire = serde_json::from_slice(&decoded).map_err(|_| {
        SponsorshipStoreError::Malformed("sponsorship recovery cursor is not usable".into())
    })?;
    if cursor.version != 1
        || cursor.domain != domain
        || layer2_core::seconds_from_rfc3339(&cursor.updated_at).is_none()
        || cursor.id.is_empty()
        || cursor.id.len() > 512
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship recovery cursor is not usable".into(),
        ));
    }
    Ok(Some((cursor.updated_at, cursor.id)))
}

fn recovery_page_filter(
    base: Document,
    page: &SponsorshipRecoveryPageRequest,
    domain: &str,
) -> Result<Document, SponsorshipStoreError> {
    recovery_page_filter_on(base, page, domain, "updated_at")
}

fn recovery_page_filter_on(
    base: Document,
    page: &SponsorshipRecoveryPageRequest,
    domain: &str,
    ordered_field: &str,
) -> Result<Document, SponsorshipStoreError> {
    let Some((ordered_at, id)) = decode_recovery_cursor(page, domain)? else {
        return Ok(base);
    };
    let mut strictly_later = Document::new();
    strictly_later.insert(ordered_field, doc! { "$gt": &ordered_at });
    let mut same_time_later_id = Document::new();
    same_time_later_id.insert(ordered_field, &ordered_at);
    same_time_later_id.insert("_id", doc! { "$gt": &id });
    Ok(doc! {
        "$and": [
            base,
            {
                "$or": [
                    strictly_later,
                    same_time_later_id,
                ],
            },
        ],
    })
}

fn encode_recovery_cursor(
    domain: &str,
    updated_at: &str,
    id: &str,
) -> Result<String, SponsorshipStoreError> {
    let encoded = serde_json::to_vec(&RecoveryCursorWire {
        version: 1,
        domain: domain.to_string(),
        updated_at: updated_at.to_string(),
        id: id.to_string(),
    })
    .map_err(|_| {
        SponsorshipStoreError::Malformed("sponsorship recovery cursor cannot be encoded".into())
    })?;
    let cursor = URL_SAFE_NO_PAD.encode(encoded);
    if cursor.len() > MAX_SPONSORSHIP_RECOVERY_CURSOR_BYTES {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship recovery cursor exceeds its size bound".into(),
        ));
    }
    Ok(cursor)
}

fn finish_recovery_page<T>(
    mut items: Vec<T>,
    page: &SponsorshipRecoveryPageRequest,
    domain: &str,
    key: impl Fn(&T) -> (&str, &str),
) -> Result<SponsorshipRecoveryPage<T>, SponsorshipStoreError> {
    let limit = page.limit() as usize;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next = has_more
        .then(|| {
            let (updated_at, id) = key(items.last().expect("a full recovery page has a final row"));
            encode_recovery_cursor(domain, updated_at, id)
        })
        .transpose()?;
    Ok(SponsorshipRecoveryPage { items, next })
}

fn recovery_query_limit(page: &SponsorshipRecoveryPageRequest) -> i64 {
    i64::from(page.limit()) + 1
}

fn rewrap_kind_code(kind: SponsorshipCiphertextRecordKind) -> &'static str {
    match kind {
        SponsorshipCiphertextRecordKind::Credential => "credential",
        SponsorshipCiphertextRecordKind::GrantActivation => "grant_activation",
        SponsorshipCiphertextRecordKind::Grant => "grant",
        SponsorshipCiphertextRecordKind::UsageReservation => "usage_reservation",
    }
}

fn rewrap_domain(kind: SponsorshipCiphertextRecordKind, retiring_key_id: &str) -> String {
    format!("rewrap:{}:{retiring_key_id}", rewrap_kind_code(kind))
}

const REWRAP_CURSOR_STAMP: &str = "1970-01-01T00:00:00Z";

fn rewrap_page_filter(
    base: Document,
    page: &SponsorshipRecoveryPageRequest,
    domain: &str,
) -> Result<Document, SponsorshipStoreError> {
    let Some((stamp, id)) = decode_recovery_cursor(page, domain)? else {
        return Ok(base);
    };
    if stamp != REWRAP_CURSOR_STAMP {
        return Err(SponsorshipStoreError::Malformed(
            "sponsorship rewrap cursor is not usable".into(),
        ));
    }
    Ok(doc! {
        "$and": [
            base,
            { "_id": { "$gt": id } },
        ],
    })
}

fn finish_rewrap_page(
    mut items: Vec<SponsorshipCiphertextRewrapCandidate>,
    page: &SponsorshipRecoveryPageRequest,
    domain: &str,
) -> Result<SponsorshipRecoveryPage<SponsorshipCiphertextRewrapCandidate>, SponsorshipStoreError> {
    let limit = page.limit() as usize;
    let has_more = items.len() > limit;
    items.truncate(limit);
    let next = has_more
        .then(|| {
            encode_recovery_cursor(
                domain,
                REWRAP_CURSOR_STAMP,
                &items
                    .last()
                    .expect("a full rewrap page has a final row")
                    .record_id,
            )
        })
        .transpose()?;
    Ok(SponsorshipRecoveryPage { items, next })
}

fn envelope_bson(envelope: &SponsorshipCipherEnvelope) -> Result<Bson, SponsorshipStoreError> {
    mongodb::bson::serialize_to_bson(envelope).map_err(|_| {
        SponsorshipStoreError::Malformed(
            "rewrapped sponsorship ciphertext cannot be serialized".into(),
        )
    })
}

fn rewrap_document_fingerprint(
    kind: SponsorshipCiphertextRecordKind,
    document: &Document,
) -> Result<String, SponsorshipStoreError> {
    let encoded = mongodb::bson::serialize_to_vec(document).map_err(|_| {
        SponsorshipStoreError::Malformed("sponsorship rewrap snapshot cannot be serialized".into())
    })?;
    let mut hasher = Sha256::new();
    hasher.update(b"cma-sponsorship-rewrap-document-v1");
    hasher.update([0]);
    hasher.update(rewrap_kind_code(kind).as_bytes());
    hasher.update([0]);
    hasher.update((encoded.len() as u64).to_be_bytes());
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

fn decode_rewrap_document<T: serde::de::DeserializeOwned>(
    document: Document,
    resource: &str,
) -> Result<T, SponsorshipStoreError> {
    mongodb::bson::deserialize_from_document(document).map_err(|_| {
        SponsorshipStoreError::Malformed(format!(
            "stored {resource} cannot be decoded for ciphertext rewrap"
        ))
    })
}

fn rewrap_exact_document_filter(
    document: &Document,
    record_id: &str,
    retiring_key_id: &str,
) -> Document {
    doc! {
        "_id": record_id,
        "encryption_key_id": retiring_key_id,
        "$expr": {
            "$eq": [
                "$$ROOT",
                { "$literal": Bson::Document(document.clone()) },
            ],
        },
    }
}

#[async_trait::async_trait]
impl SponsorshipStore for MongoSponsorshipStore {
    async fn gate_fence(&self) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
        let document = self
            .gate
            .find_one(doc! { "_id": GATE_ID })
            .await
            .map_err(|error| stored_read_failure("read sponsorship gate", error))?
            .ok_or_else(|| {
                SponsorshipStoreError::Malformed("the sponsorship gate is missing".into())
            })?;
        read_gate(document)
    }

    async fn engage_kill_switch(
        &self,
        expected_epoch: u64,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
        validate_audit_event(&audit)?;
        let (gate, engagement_id, drain_audit) = self
            .transaction((self.clone(), audit), |session, (store, audit)| {
                async move {
                    let next_epoch = expected_epoch.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "sponsorship gate epoch exhausted".into(),
                        ))
                    })?;
                    let document = store
                        .gate
                        .find_one_and_update(
                            doc! {
                                "_id": GATE_ID,
                                "epoch": signed(expected_epoch, "sponsorship gate epoch")
                                    .map_err(custom)?,
                                "kill_switch_engaged": false,
                            },
                            doc! {
                                "$set": {
                                    "epoch": signed(next_epoch, "sponsorship gate epoch")
                                        .map_err(custom)?,
                                    "kill_switch_engaged": true,
                                    "kill_switch_engagement_id": &audit.event_id,
                                    "kill_switch_drain_complete": false,
                                },
                                "$inc": { "authority_read_revision": 1_i64 },
                            },
                        )
                        .return_document(ReturnDocument::After)
                        .session(&mut *session)
                        .await?;
                    if let Some(document) = document {
                        let gate = read_gate(document).map_err(custom)?;
                        store.audit_in_transaction(session, audit).await?;
                        return Ok((gate, audit.event_id.clone(), audit.clone()));
                    }

                    let document = store
                        .gate
                        .find_one(doc! { "_id": GATE_ID })
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "the sponsorship gate is missing".into(),
                            ))
                        })?;
                    let gate = read_gate(document.clone()).map_err(custom)?;
                    let exact_epoch = gate.epoch == expected_epoch
                        || gate.epoch == expected_epoch.saturating_add(1);
                    if !gate.kill_switch_engaged || !exact_epoch {
                        return Err(custom(SponsorshipStoreError::GateChanged));
                    }
                    let engagement_id = document.kill_switch_engagement_id.ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "engaged kill switch has no durable identity".into(),
                        ))
                    })?;
                    let stored_audit = store
                        .audits
                        .find_one(doc! { "_id": &engagement_id })
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "engaged kill switch has no caller audit".into(),
                            ))
                        })?;
                    let stored_audit = read_audit(stored_audit).map_err(custom)?;
                    Ok((gate, engagement_id, stored_audit))
                }
                .boxed()
            })
            .await?;
        match self
            .drain_kill_switch_engagement(&engagement_id, &drain_audit)
            .await
        {
            Ok(true) => {}
            Ok(false) => {
                return Err(SponsorshipStoreError::Unavailable(format!(
                    "kill switch engagement '{engagement_id}' is durable but its bounded activation drain remains pending"
                )));
            }
            Err(error) => {
                tracing::warn!(%error, %engagement_id, "kill switch engaged; bounded activation drain remains pending");
                return Err(error);
            }
        }
        Ok(gate)
    }

    async fn launch_authority(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredLaunchAuthority>, SponsorshipStoreError> {
        self.transaction(
            (self.clone(), launch_id.clone()),
            |session, (store, launch_id)| {
                async move {
                    let Some(launch) = store.launch_in_transaction(session, launch_id).await?
                    else {
                        return Ok(None);
                    };
                    let policy_id = launch_sponsorship_policy_id(&launch).map_err(custom)?;
                    let (policy, _) = store
                        .policy_in_transaction(session, &policy_id)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "the durable Launch names no sponsorship policy".into(),
                            ))
                        })?;
                    SponsoredLaunchAuthority::from_launch(&launch, &policy)
                        .map(Some)
                        .map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })
                }
                .boxed()
            },
        )
        .await
    }

    async fn credential_registration(
        &self,
        identity: &SponsoredCredentialRegistrationIdentity,
    ) -> Result<Option<SponsoredCredentialRecord>, SponsorshipStoreError> {
        validate_operational_aliases(
            &self.operational,
            &identity.request.encoded_candidates(),
            "credential registration request candidate",
        )?;
        validate_operational_aliases(
            &self.operational,
            &identity.command.encoded_candidates(),
            "credential registration command candidate",
        )?;
        self.transaction(
            (self.clone(), identity.clone()),
            |session, (store, identity)| {
                async move {
                    let Some(binding) = store
                        .credential_registration_binding_in_transaction(session, &identity.request)
                        .await?
                    else {
                        return Ok(None);
                    };
                    if !aliases_match_candidates(&binding.command_aliases, &identity.command) {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let request_aliases = identity.request.encoded_candidates();
                    let command_aliases = identity.command.encoded_candidates();
                    if binding.request_aliases != request_aliases
                        || binding.command_aliases != command_aliases
                    {
                        let updated = store
                            .credential_registrations
                            .update_one(
                                doc! { "_id": &binding.id },
                                doc! {
                                    "$set": {
                                        "request_aliases": request_aliases,
                                        "command_aliases": command_aliases,
                                    }
                                },
                            )
                            .session(&mut *session)
                            .await?;
                        if updated.matched_count != 1 {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                    }
                    let credential_id = SponsoredCredentialId::parse(binding.credential_id)
                        .map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })?;
                    let stored = store
                        .credential_in_transaction(session, &credential_id)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "credential registration points to no record".into(),
                            ))
                        })?;
                    Ok(Some(stored.record))
                }
                .boxed()
            },
        )
        .await
    }

    async fn insert_credential(
        &self,
        identity: &SponsoredCredentialRegistrationIdentity,
        provider_identity: &SponsorshipOperationalPseudonymSet,
        record: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        validate_operational_aliases(
            &self.operational,
            &identity.request.encoded_candidates(),
            "credential registration request candidate",
        )?;
        validate_operational_aliases(
            &self.operational,
            &identity.command.encoded_candidates(),
            "credential registration command candidate",
        )?;
        let provider_aliases = provider_identity.encoded_candidates();
        validate_operational_aliases(
            &self.operational,
            &provider_aliases,
            "provider credential identity candidate",
        )?;
        if record.authority.provider_credential_id_digest != provider_identity.current().as_str() {
            return Err(SponsorshipStoreError::Malformed(
                "credential canonical provider identity is not the current candidate".into(),
            ));
        }
        if record.generation != 1
            || record.state != SponsoredCredentialState::Active
            || record.material_state != SponsoredCredentialMaterialState::Retained
            || record.revoked_at.is_some()
            || record.pending_operation.is_some()
            || record.superseded_operation.is_some()
            || record.last_completed_operation.is_some()
            || record.created_at != record.updated_at
        {
            return Err(SponsorshipStoreError::Malformed(
                "credential registration identity or initial lifecycle shape is invalid".into(),
            ));
        }
        let secret = Zeroizing::new(secret.expose().to_string());
        for attempt in 0..2 {
            let result = self
                .transaction(
                    (
                        self.clone(),
                        identity.clone(),
                        provider_identity.clone(),
                        record.clone(),
                        secret.clone(),
                        audit.clone(),
                    ),
                    |session, (store, identity, provider_identity, candidate, secret, audit)| {
                        async move {
                            if let Some(binding) = store
                                .credential_registration_binding_in_transaction(
                                    session,
                                    &identity.request,
                                )
                                .await?
                            {
                                if !aliases_match_candidates(
                                    &binding.command_aliases,
                                    &identity.command,
                                ) {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                                let updated = store
                                    .credential_registrations
                                    .update_one(
                                        doc! { "_id": &binding.id },
                                        doc! {
                                            "$set": {
                                                "request_aliases": identity.request.encoded_candidates(),
                                                "command_aliases": identity.command.encoded_candidates(),
                                            }
                                        },
                                    )
                                    .session(&mut *session)
                                    .await?;
                                if updated.matched_count != 1 {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                                let id = SponsoredCredentialId::parse(binding.credential_id)
                                    .map_err(|error| {
                                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                                    })?;
                                let stored = store
                                    .credential_in_transaction(session, &id)
                                    .await?
                                    .ok_or_else(|| {
                                        custom(SponsorshipStoreError::Malformed(
                                            "credential registration points to no record".into(),
                                        ))
                                    })?;
                                if credential_registration_bindings_match(
                                    &stored.record,
                                    candidate,
                                ) {
                                    if !provider_identity.iter().any(|identity| {
                                        identity.as_str()
                                            == stored
                                                .record
                                                .authority
                                                .provider_credential_id_digest
                                    }) {
                                        return Err(custom(SponsorshipStoreError::Conflict));
                                    }
                                    let migrated = store
                                        .credentials
                                        .update_one(
                                            doc! { "_id": &stored.record.id.0 },
                                            doc! {
                                                "$set": {
                                                    "provider_credential_id_aliases":
                                                        provider_identity.encoded_candidates(),
                                                }
                                            },
                                        )
                                        .session(&mut *session)
                                        .await?;
                                    if migrated.matched_count != 1 {
                                        return Err(custom(SponsorshipStoreError::Conflict));
                                    }
                                    return Ok(stored.record);
                                }
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }

                            if let Some(document) = store
                                .provider_credential_binding_in_transaction(
                                    session,
                                    &candidate.authority.provider,
                                    provider_identity,
                                )
                                .await?
                            {
                                if !provider_identity.iter().any(|identity| {
                                    identity.as_str()
                                        == document.provider_credential_id_digest
                                }) {
                                    return Err(custom(SponsorshipStoreError::Malformed(
                                        "provider credential aliases do not bind their canonical authority"
                                            .into(),
                                    )));
                                }
                                let id = SponsoredCredentialId::parse(document.id).map_err(
                                    |error| {
                                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                                    },
                                )?;
                                let stored = store
                                    .credential_in_transaction(session, &id)
                                    .await?
                                    .ok_or_else(|| {
                                        custom(SponsorshipStoreError::Malformed(
                                            "credential provider identity points to no record"
                                                .into(),
                                        ))
                                    })?;
                                if !credential_registration_bindings_match(
                                    &stored.record,
                                    candidate,
                                ) || stored.current_secret.as_ref().is_none_or(|stored_secret| {
                                    stored_secret.expose() != secret.as_str()
                                }) {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                                let migrated = store
                                    .credentials
                                    .update_one(
                                        doc! { "_id": &stored.record.id.0 },
                                        doc! {
                                            "$set": {
                                                "provider_credential_id_aliases":
                                                    provider_identity.encoded_candidates(),
                                            }
                                        },
                                    )
                                    .session(&mut *session)
                                    .await?;
                                if migrated.matched_count != 1 {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                                store
                                    .credential_registrations
                                    .insert_one(credential_registration_doc(
                                        identity,
                                        &stored.record.id,
                                        &candidate.created_at,
                                    ))
                                    .session(&mut *session)
                                    .await?;
                                return Ok(stored.record);
                            }

                            if let Some(stored) = store
                                .credential_in_transaction(session, &candidate.id)
                                .await?
                            {
                                if stored.record == *candidate
                                    && stored.current_secret.as_ref().is_some_and(|stored_secret| {
                                        stored_secret.expose() == secret.as_str()
                                    })
                                {
                                    let migrated = store
                                        .credentials
                                        .update_one(
                                            doc! { "_id": &candidate.id.0 },
                                            doc! {
                                                "$set": {
                                                    "provider_credential_id_aliases":
                                                        provider_identity.encoded_candidates(),
                                                }
                                            },
                                        )
                                        .session(&mut *session)
                                        .await?;
                                    if migrated.matched_count != 1 {
                                        return Err(custom(SponsorshipStoreError::Conflict));
                                    }
                                    store
                                        .credential_registrations
                                        .insert_one(credential_registration_doc(
                                            identity,
                                            &candidate.id,
                                            &candidate.created_at,
                                        ))
                                        .session(&mut *session)
                                        .await?;
                                    return Ok(stored.record);
                                }
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }

                            let document = credential_doc(
                                &store.cipher,
                                candidate,
                                &provider_identity.encoded_candidates(),
                                Some(secret.as_str()),
                                None,
                                0,
                            )
                            .map_err(custom)?;
                            store
                                .credentials
                                .insert_one(document)
                                .session(&mut *session)
                                .await?;
                            store
                                .credential_registrations
                                .insert_one(credential_registration_doc(
                                    identity,
                                    &candidate.id,
                                    &candidate.created_at,
                                ))
                                .session(&mut *session)
                                .await?;
                            store.audit_in_transaction(session, audit).await?;
                            Ok(candidate.clone())
                        }
                        .boxed()
                    },
                )
                .await;
            match result {
                Err(SponsorshipStoreError::Conflict) if attempt == 0 => continue,
                result => return result,
            }
        }
        unreachable!("the bounded credential insertion loop always returns")
    }

    async fn credential_record(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<SponsoredCredentialRecord>, SponsorshipStoreError> {
        let document = self
            .credentials
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored credential metadata", error))?;
        document
            .as_ref()
            .map(|document| read_credential_record(&self.cipher, &self.operational, document))
            .transpose()
    }

    async fn credentials_for_owner(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredCredentialRecord>, SponsorshipStoreError> {
        validate_subject_query(owner)?;
        let filter = newest_page_filter(doc! { "owner_subject": &owner.0 }, page)?;
        let documents = self
            .credentials
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(page_query_limit(page))
            .await
            .map_err(|error| stored_read_failure("list owner sponsorship credentials", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| stored_read_failure("list owner sponsorship credentials", error))?;
        let mut records = Vec::with_capacity(documents.len());
        for document in documents {
            if document.owner_subject != owner.0 {
                return Err(SponsorshipStoreError::Malformed(
                    "owner credential index crossed its subject boundary".into(),
                ));
            }
            records.push(read_credential_record(
                &self.cipher,
                &self.operational,
                &document,
            )?);
        }
        Ok(finish_newest_page(records, page, |record| {
            (&record.created_at, &record.id.0)
        }))
    }

    async fn credential(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<StoredSponsoredCredential>, SponsorshipStoreError> {
        let document = self
            .credentials
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored credential", error))?;
        document
            .map(|document| {
                let decoded = read_credential(&self.cipher, &self.operational, document)?;
                let stored = StoredSponsoredCredential {
                    record: decoded.record,
                    secret: decoded.current_secret,
                };
                stored.validate()?;
                Ok(stored)
            })
            .transpose()
    }

    async fn completed_credential_operation(
        &self,
        id: &SponsoredCredentialId,
        kind: ProviderOperationKind,
        client_request_identity: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<CompletedProviderOperation>, SponsorshipStoreError> {
        validate_operational_aliases(
            &self.operational,
            &client_request_identity.encoded_candidates(),
            "credential operation request candidate",
        )?;
        let documents = self
            .credential_operations
            .find(doc! {
                "_id.credential_id": &id.0,
                "_id.operation_kind": operation_kind_code(kind),
                "_id.client_request_id_digest": {
                    "$in": client_request_identity.encoded_candidates(),
                },
            })
            .limit((MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS + 2) as i64)
            .await
            .map_err(|error| {
                stored_read_failure("read sponsored credential operation replay", error)
            })?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| {
                stored_read_failure("read sponsored credential operation replay", error)
            })?;
        if documents.len() > 1 {
            return Err(SponsorshipStoreError::Malformed(
                "credential operation candidates resolve multiple durable operations".into(),
            ));
        }
        documents
            .into_iter()
            .next()
            .map(|document| {
                if document.id.credential_id != id.0 {
                    return Err(SponsorshipStoreError::Malformed(
                        "credential operation replay identity is inconsistent".into(),
                    ));
                }
                self.operational
                    .parse_configured(document.id.client_request_id_digest.clone())
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                read_ledger(document)
            })
            .transpose()
    }

    async fn replace_credential(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        secret: Option<&ProviderCredentialSecret>,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        require_successor_generation(expected_generation, record.generation, "credential")?;
        let secret = secret.map(|secret| Zeroizing::new(secret.expose().to_string()));
        self.transaction(
            (
                self.clone(),
                expected_generation,
                record.clone(),
                secret,
                audit,
            ),
            |session, (store, expected, candidate, supplied_secret, audit)| {
                async move {
                    let current = store
                        .credential_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if current.record.generation != *expected {
                        return if current.record == *candidate {
                            Ok(current.record)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
                    }
                    if !credential_update_is_legal(&current.record, candidate) {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if credential_revocation_attempt_begins(&current.record, candidate)
                        && store
                            .reservations
                            .find_one(doc! {
                                "credential_id": &candidate.id.0,
                                "state": { "$in": ["reserved", "attempting", "ambiguous"] },
                                "model_dispatch_payload": { "$exists": true },
                            })
                            .session(&mut *session)
                            .await?
                            .is_some()
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let current_secret = supplied_secret
                        .as_ref()
                        .map(|secret| secret.as_str())
                        .or_else(|| {
                            current
                                .current_secret
                                .as_ref()
                                .map(|secret| secret.expose())
                        });
                    let replacement = credential_doc(
                        &store.cipher,
                        candidate,
                        &current.provider_credential_id_aliases,
                        current_secret,
                        current
                            .pending_secret
                            .as_ref()
                            .map(|secret| secret.expose()),
                        current.authority_read_revision,
                    )
                    .map_err(custom)?;
                    let result = store
                        .credentials
                        .replace_one(
                            doc! {
                                "_id": &candidate.id.0,
                                "generation": signed(*expected, "credential generation")
                                    .map_err(custom)?,
                                "authority_read_revision": current.authority_read_revision,
                            },
                            replacement,
                        )
                        .session(&mut *session)
                        .await?;
                    if result.modified_count != 1 {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    store.audit_in_transaction(session, audit).await?;
                    Ok(candidate.clone())
                }
                .boxed()
            },
        )
        .await
    }

    async fn stage_credential_rotation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        replacement: &ProviderCredentialSecret,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        require_successor_generation(expected_generation, record.generation, "credential")?;
        let pending = record
            .pending_operation
            .as_ref()
            .filter(|operation| operation.kind == ProviderOperationKind::RotateCredential)
            .ok_or(SponsorshipStoreError::Conflict)?;
        let replacement = Zeroizing::new(replacement.expose().to_string());
        self.transaction(
            (
                self.clone(),
                expected_generation,
                record.clone(),
                pending.client_request_id_digest.clone(),
                replacement,
                audit,
            ),
            |session, (store, expected, candidate, request_digest, replacement, audit)| {
                async move {
                    if store
                        .credential_operations
                        .find_one(doc! {
                            "_id.credential_id": &candidate.id.0,
                            "_id.operation_kind": "rotate_credential",
                            "_id.client_request_id_digest": &*request_digest,
                        })
                        .session(&mut *session)
                        .await?
                        .is_some()
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let current = store
                        .credential_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if current.record.generation != *expected {
                        return if current.record == *candidate {
                            Ok(current.record)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
                    }
                    if !credential_rotation_stage_is_legal(&current.record, candidate) {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let current_secret = current.current_secret.as_ref().ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "current credential secret is missing".into(),
                        ))
                    })?;
                    let replacement_doc = credential_doc(
                        &store.cipher,
                        candidate,
                        &current.provider_credential_id_aliases,
                        Some(current_secret.expose()),
                        Some(replacement.as_str()),
                        current.authority_read_revision,
                    )
                    .map_err(custom)?;
                    let result = store
                        .credentials
                        .replace_one(
                            doc! {
                                "_id": &candidate.id.0,
                                "generation": signed(*expected, "credential generation")
                                    .map_err(custom)?,
                                "authority_read_revision": current.authority_read_revision,
                            },
                            replacement_doc,
                        )
                        .session(&mut *session)
                        .await?;
                    if result.modified_count != 1 {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    store.audit_in_transaction(session, audit).await?;
                    Ok(candidate.clone())
                }
                .boxed()
            },
        )
        .await
    }

    async fn pending_credential_rotation_secret(
        &self,
        id: &SponsoredCredentialId,
    ) -> Result<Option<ProviderCredentialSecret>, SponsorshipStoreError> {
        let document = self
            .credentials
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read pending credential rotation", error))?;
        document
            .map(|document| {
                read_credential(&self.cipher, &self.operational, document)
                    .map(|decoded| decoded.pending_secret)
            })
            .transpose()
            .map(Option::flatten)
    }

    async fn complete_credential_rotation(
        &self,
        expected_generation: u64,
        provider_identity: &SponsorshipOperationalPseudonymSet,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        self.complete_credential_mutation(
            expected_generation,
            Some(provider_identity),
            record,
            audit,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Confirmed,
        )
        .await
    }

    async fn abort_credential_rotation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        self.complete_credential_mutation(
            expected_generation,
            None,
            record,
            audit,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Rejected,
        )
        .await
    }

    async fn complete_credential_revocation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        self.complete_credential_mutation(
            expected_generation,
            None,
            record,
            audit,
            ProviderOperationKind::RevokeCredential,
            CompletedProviderOperationOutcome::Confirmed,
        )
        .await
    }

    async fn policy_creation(
        &self,
        identity: &SponsorshipPolicyCreationIdentity,
    ) -> Result<Option<SponsorshipPolicy>, SponsorshipStoreError> {
        validate_operational_aliases(
            &self.operational,
            &identity.request.encoded_candidates(),
            "policy creation request candidate",
        )?;
        validate_operational_aliases(
            &self.operational,
            &identity.command.encoded_candidates(),
            "policy creation command candidate",
        )?;
        if !is_digest(&identity.payload_digest) {
            return Err(SponsorshipStoreError::Malformed(
                "policy creation payload digest is malformed".into(),
            ));
        }
        self.transaction(
            (self.clone(), identity.clone()),
            |session, (store, identity)| {
                async move {
                    let Some(binding) = store
                        .policy_creation_binding_in_transaction(session, &identity.request)
                        .await?
                    else {
                        return Ok(None);
                    };
                    if binding.payload_digest != identity.payload_digest
                        || !aliases_match_candidates(&binding.command_aliases, &identity.command)
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let request_aliases = identity.request.encoded_candidates();
                    let command_aliases = identity.command.encoded_candidates();
                    if binding.request_aliases != request_aliases
                        || binding.command_aliases != command_aliases
                    {
                        let updated = store
                            .policy_creations
                            .update_one(
                                doc! { "_id": &binding.id },
                                doc! {
                                    "$set": {
                                        "request_aliases": request_aliases,
                                        "command_aliases": command_aliases,
                                    }
                                },
                            )
                            .session(&mut *session)
                            .await?;
                        if updated.matched_count != 1 {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                    }
                    let policy_id =
                        SponsorshipPolicyId::parse(binding.policy_id).map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })?;
                    let (policy, _) = store
                        .policy_in_transaction(session, &policy_id)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "policy creation identity points to no policy".into(),
                            ))
                        })?;
                    Ok(Some(policy))
                }
                .boxed()
            },
        )
        .await
    }

    async fn insert_policy(
        &self,
        command: PersistSponsorshipPolicy,
    ) -> Result<SponsorshipPolicy, SponsorshipStoreError> {
        command
            .policy
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_gate(&command.gate_fence)?;
        validate_audit_event(&command.audit)?;
        if command.policy.generation != 1
            || command.policy.state != layer2_core::SponsorshipPolicyState::Active
            || command.policy.revoked_at.is_some()
            || command.policy.provider_policy.provider_policy_key
                != command.gate_fence.trusted_provider_policy_key
        {
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        validate_operational_aliases(
            &self.operational,
            &command.idempotency.request.encoded_candidates(),
            "policy creation request candidate",
        )?;
        validate_operational_aliases(
            &self.operational,
            &command.idempotency.command.encoded_candidates(),
            "policy creation command candidate",
        )?;
        if !is_digest(&command.idempotency.payload_digest) {
            return Err(SponsorshipStoreError::Malformed(
                "policy creation payload digest is malformed".into(),
            ));
        }
        let sponsor_key = command
            .policy
            .owner
            .sponsorship_authority_key()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        self.ensure_sponsor_counter(&sponsor_key).await?;
        let usage = SponsorshipPolicyUsage {
            consumed: Default::default(),
            reserved: Default::default(),
            active_launches: 0,
        };
        let candidate = policy_doc(&command.policy, &usage)?;
        let idempotency = command.idempotency.clone();
        for attempt in 0..2 {
            let result = self
                .transaction(
                    (
                        self.clone(),
                        candidate.clone(),
                        command.clone(),
                        sponsor_key.clone(),
                        idempotency.clone(),
                    ),
                    |session, (store, candidate, command, sponsor_key, idempotency)| {
                        async move {
                            if let Some(binding) = store
                                .policy_creation_binding_in_transaction(
                                    session,
                                    &idempotency.request,
                                )
                                .await?
                            {
                                if binding.payload_digest != idempotency.payload_digest
                                    || !aliases_match_candidates(
                                        &binding.command_aliases,
                                        &idempotency.command,
                                    )
                                {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                                let request_aliases = idempotency.request.encoded_candidates();
                                let command_aliases = idempotency.command.encoded_candidates();
                                if binding.request_aliases != request_aliases
                                    || binding.command_aliases != command_aliases
                                {
                                    let updated = store
                                        .policy_creations
                                        .update_one(
                                            doc! { "_id": &binding.id },
                                            doc! {
                                                "$set": {
                                                    "request_aliases": request_aliases,
                                                    "command_aliases": command_aliases,
                                                }
                                            },
                                        )
                                        .session(&mut *session)
                                        .await?;
                                    if updated.matched_count != 1 {
                                        return Err(custom(SponsorshipStoreError::Conflict));
                                    }
                                }
                                let policy_id = SponsorshipPolicyId::parse(binding.policy_id)
                                    .map_err(|error| {
                                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                                    })?;
                                let (stored, _) = store
                                    .policy_in_transaction(session, &policy_id)
                                    .await?
                                    .ok_or_else(|| {
                                        custom(SponsorshipStoreError::Malformed(
                                            "policy creation identity points to no policy".into(),
                                        ))
                                    })?;
                                return Ok(stored);
                            }
                            if let Some((stored, _)) = store
                                .policy_in_transaction(session, &command.policy.id)
                                .await?
                            {
                                if stored != command.policy {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                                let counter = store
                                    .policy_counters
                                    .find_one(doc! { "_id": &command.policy.id.0 })
                                    .session(&mut *session)
                                    .await?
                                    .ok_or_else(|| {
                                        custom(SponsorshipStoreError::Malformed(
                                            "sponsorship policy rate counter is missing".into(),
                                        ))
                                    })?;
                                validate_document_version(
                                    counter.document_schema_version,
                                    "sponsorship policy rate counter",
                                )
                                .map_err(custom)?;
                                if counter.policy_id != command.policy.id.0
                                    || counter.rate_revision < 0
                                {
                                    return Err(custom(SponsorshipStoreError::Malformed(
                                        "sponsorship policy rate counter is invalid".into(),
                                    )));
                                }
                                return Err(custom(SponsorshipStoreError::Malformed(
                                    "existing sponsorship policy has no creation identity".into(),
                                )));
                            }
                            let now = store.database_now(session).await?;
                            let now_string = database_time_string(now);
                            store
                                .require_gate_in_transaction(session, &command.gate_fence)
                                .await?;
                            command
                                .gate_fence
                                .runtime_limits
                                .require_policy_at(&command.policy, &now_string)
                                .map_err(|_| custom(SponsorshipStoreError::Conflict))?;
                            store
                                .advance_sponsor_admission_fence_in_transaction(
                                    session,
                                    sponsor_key,
                                    SponsorAdmissionFence::Policy,
                                )
                                .await?;
                            let active_policies = store
                                .policies
                                .count_documents(doc! {
                                    "sponsor_key": sponsor_key,
                                    "state": "active",
                                    "expires_at_database": { "$gt": now },
                                })
                                .session(&mut *session)
                                .await?;
                            if active_policies
                                >= u64::from(
                                    command
                                        .gate_fence
                                        .runtime_limits
                                        .maximum_active_policies_per_sponsor,
                                )
                            {
                                return Err(custom(SponsorshipStoreError::ActivePolicyLimit));
                            }
                            let credential = store
                                .credential_record_in_transaction(
                                    session,
                                    &command.policy.credential_id,
                                )
                                .await?
                                .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                            if credential.state != SponsoredCredentialState::Active
                                || credential.material_state
                                    != SponsoredCredentialMaterialState::Retained
                                || credential.owner != command.policy.owner
                                || credential.authority.provider
                                    != command.policy.provider_policy.provider
                                || credential.authority.provider_policy_key
                                    != command.policy.provider_policy.provider_policy_key
                            {
                                return Err(custom(SponsorshipStoreError::GrantUnavailable));
                            }
                            store
                                .policies
                                .insert_one(candidate.clone())
                                .session(&mut *session)
                                .await?;
                            store
                                .policy_counters
                                .insert_one(PolicyCounterDoc {
                                    policy_id: command.policy.id.0.clone(),
                                    document_schema_version: DOCUMENT_SCHEMA_VERSION,
                                    rate_revision: 0,
                                })
                                .session(&mut *session)
                                .await?;
                            store
                                .policy_creations
                                .insert_one(
                                    policy_creation_doc(
                                        &store.operational,
                                        idempotency,
                                        &command.policy.id,
                                        &command.policy.created_at,
                                    )
                                    .map_err(custom)?,
                                )
                                .session(&mut *session)
                                .await?;
                            store.audit_in_transaction(session, &command.audit).await?;
                            Ok(command.policy.clone())
                        }
                        .boxed()
                    },
                )
                .await;
            match result {
                Err(SponsorshipStoreError::Conflict) if attempt == 0 => continue,
                result => return result,
            }
        }
        unreachable!("the bounded policy insertion loop always returns")
    }

    async fn policy(
        &self,
        id: &SponsorshipPolicyId,
    ) -> Result<Option<SponsorshipPolicy>, SponsorshipStoreError> {
        self.policies
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsorship policy", error))?
            .map(read_policy)
            .transpose()
            .map(|value| value.map(|(policy, _)| policy))
    }

    async fn policies_for_owner(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<StoredSponsorshipPolicy>, SponsorshipStoreError> {
        validate_subject_query(owner)?;
        let filter = newest_page_filter(doc! { "owner_subject": &owner.0 }, page)?;
        let documents = self
            .policies
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(page_query_limit(page))
            .await
            .map_err(|error| stored_read_failure("list owner sponsorship policies", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| stored_read_failure("list owner sponsorship policies", error))?;
        let mut records = Vec::with_capacity(documents.len());
        for document in documents {
            if document.owner_subject != owner.0 {
                return Err(SponsorshipStoreError::Malformed(
                    "owner policy index crossed its subject boundary".into(),
                ));
            }
            let (policy, usage) = read_policy(document)?;
            let stored = StoredSponsorshipPolicy { policy, usage };
            stored.validate()?;
            records.push(stored);
        }
        Ok(finish_newest_page(records, page, |stored| {
            (&stored.policy.created_at, &stored.policy.id.0)
        }))
    }

    async fn policy_usage(
        &self,
        id: &SponsorshipPolicyId,
    ) -> Result<SponsorshipPolicyUsage, SponsorshipStoreError> {
        Ok(self
            .policies
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsorship policy usage", error))?
            .map(read_policy)
            .transpose()?
            .map(|(_, usage)| usage)
            .unwrap_or(SponsorshipPolicyUsage {
                consumed: Default::default(),
                reserved: Default::default(),
                active_launches: 0,
            }))
    }

    async fn replace_policy(
        &self,
        expected_generation: u64,
        policy: &SponsorshipPolicy,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipPolicy, SponsorshipStoreError> {
        policy
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        require_successor_generation(expected_generation, policy.generation, "policy")?;
        self.transaction(
            (self.clone(), expected_generation, policy.clone(), audit),
            |session, (store, expected, candidate, audit)| {
                async move {
                    let (current, usage) = store
                        .policy_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if current.generation != *expected {
                        return if current == *candidate {
                            Ok(current)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
                    }
                    if !policy_update_is_legal(&current, candidate) {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let exposure = usage.exposure().map_err(|error| {
                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                    })?;
                    if !exposure.quota.fits_within(candidate.aggregate_quota)
                        || exposure.spend_micros > candidate.aggregate_budget_micros
                        || usage.active_launches > candidate.max_concurrent_launches
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let replacement = policy_doc(candidate, &usage).map_err(custom)?;
                    let result = store
                        .policies
                        .replace_one(
                            doc! {
                                "_id": &candidate.id.0,
                                "policy.generation": signed(*expected, "policy generation")
                                    .map_err(custom)?,
                            },
                            replacement,
                        )
                        .session(&mut *session)
                        .await?;
                    if result.modified_count != 1 {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    store.audit_in_transaction(session, audit).await?;
                    Ok(candidate.clone())
                }
                .boxed()
            },
        )
        .await
    }

    async fn insert_acceptance(
        &self,
        command: PersistSponsoredAcceptance,
    ) -> Result<SponsoredGrantAcceptance, SponsorshipStoreError> {
        command
            .acceptance
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&command.audit)?;
        let replay_candidate = command.acceptance.clone();
        let replay_candidates = command.idempotency_candidates.clone();
        let result = self
            .transaction((self.clone(), command), |session, (store, command)| {
                async move {
                    if let Some(stored_document) = store
                        .acceptances
                        .find_one(doc! { "_id": &command.acceptance.launch_id.0 })
                        .session(&mut *session)
                        .await?
                    {
                        let stored_aliases = stored_document.idempotency_aliases.clone();
                        let stored =
                            read_acceptance(&store.operational, stored_document).map_err(custom)?;
                        if stored.contributor != command.acceptance.contributor {
                            return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                        }
                        if acceptance_identity_matches(&stored, &command.idempotency_candidates) {
                            let aliases = command.idempotency_candidates.encoded_candidates();
                            if stored_aliases != aliases {
                                let updated = store
                                    .acceptances
                                    .update_one(
                                        doc! {
                                            "_id": &stored.launch_id.0,
                                            "acceptance.idempotency_key_hash":
                                                &stored.idempotency_key_hash,
                                        },
                                        doc! { "$set": { "idempotency_aliases": aliases } },
                                    )
                                    .session(&mut *session)
                                    .await?;
                                if updated.matched_count != 1 {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                            }
                            return if acceptance_payload_fields_match(&stored, &command.acceptance)
                            {
                                Ok(stored)
                            } else {
                                Err(custom(SponsorshipStoreError::Conflict))
                            };
                        }
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    store
                        .require_gate_in_transaction(session, &command.gate_fence)
                        .await?;
                    let launch = store
                        .launch_in_transaction(session, &command.launch_authority.launch_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
                    let policy_id = launch_sponsorship_policy_id(&launch).map_err(custom)?;
                    if let Some(stored) = store
                        .acceptance_in_transaction(session, &command.acceptance.launch_id)
                        .await?
                    {
                        if stored.contributor != command.acceptance.contributor {
                            return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                        }
                        if acceptance_identity_matches(&stored, &command.idempotency_candidates) {
                            return if acceptance_payload_fields_match(&stored, &command.acceptance)
                            {
                                Ok(stored)
                            } else {
                                Err(custom(SponsorshipStoreError::Conflict))
                            };
                        }
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let (policy, usage) =
                        store
                            .policy_in_transaction(session, &policy_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::AcceptanceChanged))?;
                    let current_launch = SponsoredLaunchAuthority::from_launch(&launch, &policy)
                        .map_err(|_| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
                    if current_launch != command.launch_authority
                        || current_launch.state != SponsoredLaunchState::TermsReview
                        || current_launch.session_id.is_some()
                        || command.acceptance.launch_id != current_launch.launch_id
                        || command.acceptance.contributor != current_launch.contributor
                        || command.acceptance.revision_id != current_launch.revision_id
                        || command.acceptance.revision_digest != current_launch.revision_digest
                        || command.acceptance.policy_id != current_launch.policy_id
                        || command.acceptance.launch_snapshot_digest
                            != current_launch.launch_snapshot_digest
                    {
                        return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                    }
                    let terms =
                        SponsoredGrantTerms::from_policy(&policy, &usage).map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })?;
                    if command.acceptance.policy_generation != policy.generation
                        || command.acceptance.terms_digest != terms.digest
                    {
                        return Err(custom(SponsorshipStoreError::AcceptanceChanged));
                    }
                    store
                        .acceptances
                        .insert_one(
                            acceptance_doc(
                                &store.operational,
                                &command.acceptance,
                                &command.idempotency_candidates,
                            )
                            .map_err(custom)?,
                        )
                        .session(&mut *session)
                        .await?;
                    store.audit_in_transaction(session, &command.audit).await?;
                    Ok(command.acceptance.clone())
                }
                .boxed()
            })
            .await;
        if result == Err(SponsorshipStoreError::Conflict) {
            if let Some(stored) = self
                .acceptance_for_request(&replay_candidate.launch_id, &replay_candidates)
                .await?
            {
                if acceptance_identity_matches(&stored, &replay_candidates)
                    && acceptance_payload_fields_match(&stored, &replay_candidate)
                {
                    return Ok(stored);
                }
            }
        }
        result
    }

    async fn acceptance_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrantAcceptance>, SponsorshipStoreError> {
        self.acceptances
            .find_one(doc! { "_id": &launch_id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored acceptance", error))?
            .map(|document| read_acceptance(&self.operational, document))
            .transpose()
    }

    async fn acceptance_for_request(
        &self,
        launch_id: &LaunchId,
        idempotency_candidates: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<SponsoredGrantAcceptance>, SponsorshipStoreError> {
        validate_operational_aliases(
            &self.operational,
            &idempotency_candidates.encoded_candidates(),
            "acceptance idempotency candidate",
        )?;
        self.transaction(
            (
                self.clone(),
                launch_id.clone(),
                idempotency_candidates.clone(),
            ),
            |session, (store, launch_id, candidates)| {
                async move {
                    let Some(document) = store
                        .acceptances
                        .find_one(doc! { "_id": &launch_id.0 })
                        .session(&mut *session)
                        .await?
                    else {
                        return Ok(None);
                    };
                    let stored_aliases = document.idempotency_aliases.clone();
                    let acceptance =
                        read_acceptance(&store.operational, document).map_err(custom)?;
                    if acceptance_identity_matches(&acceptance, candidates) {
                        let aliases = candidates.encoded_candidates();
                        if stored_aliases != aliases {
                            let result = store
                                .acceptances
                                .update_one(
                                    doc! {
                                        "_id": &launch_id.0,
                                        "acceptance.idempotency_key_hash":
                                            &acceptance.idempotency_key_hash,
                                    },
                                    doc! { "$set": { "idempotency_aliases": aliases } },
                                )
                                .session(&mut *session)
                                .await?;
                            if result.matched_count != 1 {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                        }
                    }
                    Ok(Some(acceptance))
                }
                .boxed()
            },
        )
        .await
    }

    async fn claim_grant_activation(
        &self,
        command: ClaimGrantActivation,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError> {
        if command.candidate.lease.is_some() {
            return Err(SponsorshipStoreError::Malformed(
                "activation claim candidates cannot supply a lease; persistence must use database time"
                    .into(),
            ));
        }
        command
            .candidate
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .launch_authority
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .acceptance
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        for (candidates, kind) in [
            (
                &command.holder_candidates,
                "activation lease holder candidate",
            ),
            (
                &command.billing_request_candidates,
                "activation billing request candidate",
            ),
            (
                &command.provider_request_candidates,
                "activation provider request candidate",
            ),
        ] {
            validate_operational_aliases(
                &self.operational,
                &candidates.encoded_candidates(),
                kind,
            )?;
        }
        if command.candidate.billing_request_id
            != command.billing_request_candidates.current().as_str()
            || command.candidate.provider_request_id
                != command.provider_request_candidates.current().as_str()
            || command.candidate.revision != 1
            || command.candidate.state != GrantActivationState::AwaitingBilling
            || command.candidate.denial.is_some()
            || command.candidate.capacity_held
            || command.candidate.provider_receipt.is_some()
            || command.candidate.cleanup_operation.is_some()
            || command.candidate.created_at != command.candidate.updated_at
        {
            return Err(SponsorshipStoreError::Malformed(
                "activation claim identity or initial lifecycle shape is invalid".into(),
            ));
        }
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let now_string = database_time_string(now);
                store
                    .require_activation_dependencies_in_transaction(
                        session,
                        &command.candidate,
                        &command.launch_authority,
                        &command.acceptance,
                        &now_string,
                    )
                    .await?;
                let (admission_policy, _) = store
                    .policy_in_transaction(session, &command.candidate.policy_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                let sponsor_key = admission_policy
                    .owner
                    .sponsorship_authority_key()
                    .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
                if let Some((mut existing, lease_expiry)) = store
                    .activation_in_transaction(session, &command.candidate.launch_id)
                    .await?
                {
                    let admission = store
                        .sponsor_launch_admissions
                        .find_one(doc! { "_id": &existing.launch_id.0 })
                        .session(&mut *session)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "grant activation has no sponsor Launch admission".into(),
                            ))
                        })?;
                    validate_document_version(
                        admission.document_schema_version,
                        "sponsor Launch admission",
                    )
                    .map_err(custom)?;
                    if admission.launch_id != existing.launch_id.0
                        || admission.sponsor_key != sponsor_key
                    {
                        return Err(custom(SponsorshipStoreError::Malformed(
                            "grant activation sponsor Launch admission is invalid".into(),
                        )));
                    }
                    if !activation_claim_bindings_match(
                        &existing,
                        &command.candidate,
                        &command.billing_request_candidates,
                        &command.provider_request_candidates,
                    ) {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if matches!(
                        existing.state,
                        GrantActivationState::Committed
                            | GrantActivationState::Denied
                            | GrantActivationState::Cleaned
                    ) || lease_expiry.is_some_and(|expires| expires > now)
                    {
                        return Ok(GrantActivationClaim {
                            acquired: false,
                            intent: existing,
                        });
                    }
                    if !matches!(
                        existing.state,
                        GrantActivationState::AwaitingBilling
                            | GrantActivationState::BillingAttempting
                            | GrantActivationState::BillingAmbiguous
                            | GrantActivationState::AwaitingProvider
                            | GrantActivationState::ProviderAttempting
                            | GrantActivationState::ProviderAmbiguous
                            | GrantActivationState::ProviderConfirmed
                    ) || existing.cleanup_operation.is_some()
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if !existing.capacity_held {
                        store
                            .reserve_activation_allocation_in_transaction(session, &existing)
                            .await?;
                        existing.capacity_held = true;
                    }
                    let expected_revision = existing.revision;
                    let (lease, expires_at_database) = activation_lease(
                        command.holder_candidates.current().as_str().to_string(),
                        now,
                    )
                    .map_err(custom)?;
                    existing.lease = Some(lease);
                    existing.revision = existing.revision.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "activation revision exhausted".into(),
                        ))
                    })?;
                    existing.updated_at = now_string;
                    existing.validate().map_err(|error| {
                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                    })?;
                    store
                        .replace_activation_in_transaction(
                            session,
                            expected_revision,
                            &existing,
                            Some(expires_at_database),
                        )
                        .await?;
                    return Ok(GrantActivationClaim {
                        acquired: true,
                        intent: existing,
                    });
                }

                let mut candidate = command.candidate.clone();
                store
                    .admit_sponsor_launch_in_transaction(
                        session,
                        &candidate.launch_id,
                        &sponsor_key,
                        &candidate.gate_fence,
                        now,
                    )
                    .await?;
                store
                    .reserve_activation_allocation_in_transaction(session, &candidate)
                    .await?;
                let (lease, expires_at_database) = activation_lease(
                    command.holder_candidates.current().as_str().to_string(),
                    now,
                )
                .map_err(custom)?;
                candidate.capacity_held = true;
                candidate.lease = Some(lease);
                candidate.updated_at = now_string;
                candidate
                    .validate()
                    .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
                store
                    .activations
                    .insert_one(
                        activation_doc(&store.cipher, &candidate, Some(expires_at_database))
                            .map_err(custom)?,
                    )
                    .session(&mut *session)
                    .await?;
                Ok(GrantActivationClaim {
                    acquired: true,
                    intent: candidate,
                })
            }
            .boxed()
        })
        .await
    }

    async fn grant_activation(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<GrantActivationIntent>, SponsorshipStoreError> {
        self.activations
            .find_one(doc! { "_id": &launch_id.0 })
            .await
            .map_err(|error| stored_read_failure("read grant activation", error))?
            .map(|document| {
                read_activation(&self.cipher, &self.operational, document).map(|(intent, _)| intent)
            })
            .transpose()
    }

    async fn claim_grant_activation_cleanup(
        &self,
        command: ClaimGrantActivationCleanup,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError> {
        command
            .operation
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_operational_aliases(
            &self.operational,
            &command.holder_candidates.encoded_candidates(),
            "activation cleanup holder candidate",
        )?;
        validate_operational_aliases(
            &self.operational,
            &command
                .operation_client_request_candidates
                .encoded_candidates(),
            "activation cleanup operation candidate",
        )?;
        if command.operation.kind != ProviderOperationKind::RevokeUncommittedGrant
            || command.operation.phase != ProviderOperationPhase::Prepared
            || command.operation.client_request_id_digest
                != command
                    .operation_client_request_candidates
                    .current()
                    .as_str()
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let (mut intent, lease_expiry) = store
                    .activation_in_transaction(session, &command.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                if intent.state == GrantActivationState::Cleaned {
                    return Ok(GrantActivationClaim {
                        acquired: false,
                        intent,
                    });
                }
                if !intent.capacity_held || intent.provider_receipt.is_none() {
                    return Err(custom(SponsorshipStoreError::Conflict));
                }
                if store
                    .activation_dependencies_are_current_in_transaction(session, &intent, now)
                    .await?
                {
                    return Err(custom(SponsorshipStoreError::Conflict));
                }
                let held = lease_expiry.is_some_and(|expires| expires > now);
                match intent.state {
                    GrantActivationState::ProviderConfirmed
                    | GrantActivationState::ProviderAmbiguous => {
                        if held
                            && !intent.lease.as_ref().is_some_and(|lease| {
                                command
                                    .holder_candidates
                                    .iter()
                                    .any(|candidate| candidate.as_str() == lease.holder)
                            })
                        {
                            return Ok(GrantActivationClaim {
                                acquired: false,
                                intent,
                            });
                        }
                        intent.cleanup_operation = Some(command.operation.clone());
                        intent.state = GrantActivationState::CleanupPending;
                    }
                    GrantActivationState::CleanupPending => {
                        let existing = intent.cleanup_operation.as_ref().ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "cleanup activation has no provider operation".into(),
                            ))
                        })?;
                        if existing.kind != ProviderOperationKind::RevokeUncommittedGrant
                            || !command.operation_client_request_candidates.iter().any(
                                |candidate| candidate.as_str() == existing.client_request_id_digest,
                            )
                        {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                        if held {
                            return Ok(GrantActivationClaim {
                                acquired: false,
                                intent,
                            });
                        }
                    }
                    _ => return Err(custom(SponsorshipStoreError::Conflict)),
                }
                let expected_revision = intent.revision;
                let (lease, expires_at_database) = activation_lease(
                    command.holder_candidates.current().as_str().to_string(),
                    now,
                )
                .map_err(custom)?;
                intent.revision = intent.revision.checked_add(1).ok_or_else(|| {
                    custom(SponsorshipStoreError::Malformed(
                        "activation revision exhausted".into(),
                    ))
                })?;
                intent.lease = Some(lease);
                intent.updated_at = database_time_string(now);
                intent
                    .validate()
                    .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
                store
                    .replace_activation_in_transaction(
                        session,
                        expected_revision,
                        &intent,
                        Some(expires_at_database),
                    )
                    .await?;
                Ok(GrantActivationClaim {
                    acquired: true,
                    intent,
                })
            }
            .boxed()
        })
        .await
    }

    async fn claim_invalidated_grant_activation(
        &self,
        command: ClaimInvalidatedGrantActivation,
    ) -> Result<GrantActivationClaim, SponsorshipStoreError> {
        validate_operational_aliases(
            &self.operational,
            &command.holder_candidates.encoded_candidates(),
            "invalidated activation holder candidate",
        )?;
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let (mut intent, lease_expiry) = store
                    .activation_in_transaction(session, &command.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                if store
                    .activation_dependencies_are_current_in_transaction(session, &intent, now)
                    .await?
                    || !intent.capacity_held
                    || !matches!(
                        intent.state,
                        GrantActivationState::BillingAttempting
                            | GrantActivationState::BillingAmbiguous
                            | GrantActivationState::ProviderAttempting
                            | GrantActivationState::ProviderAmbiguous
                    )
                    || (intent.state == GrantActivationState::ProviderAmbiguous
                        && intent.provider_receipt.is_some())
                {
                    return Err(custom(SponsorshipStoreError::Conflict));
                }
                if lease_expiry.is_some_and(|expires| expires > now) {
                    return Ok(GrantActivationClaim {
                        acquired: false,
                        intent,
                    });
                }
                let expected_revision = intent.revision;
                let (lease, expires_at_database) = activation_lease(
                    command.holder_candidates.current().as_str().to_string(),
                    now,
                )
                .map_err(custom)?;
                intent.revision = intent.revision.checked_add(1).ok_or_else(|| {
                    custom(SponsorshipStoreError::Malformed(
                        "activation revision exhausted".into(),
                    ))
                })?;
                intent.lease = Some(lease);
                intent.updated_at = database_time_string(now);
                intent
                    .validate()
                    .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
                store
                    .replace_activation_in_transaction(
                        session,
                        expected_revision,
                        &intent,
                        Some(expires_at_database),
                    )
                    .await?;
                Ok(GrantActivationClaim {
                    acquired: true,
                    intent,
                })
            }
            .boxed()
        })
        .await
    }

    async fn begin_provider_grant_issue(
        &self,
        command: BeginProviderGrantIssue,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        command
            .intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .launch_authority
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .acceptance
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if command.intent.launch_id != command.launch_id
            || command.intent.revision != command.expected_revision
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let now_string = database_time_string(now);
                let dependencies = store
                    .require_activation_dependencies_in_transaction(
                        session,
                        &command.intent,
                        &command.launch_authority,
                        &command.acceptance,
                        &now_string,
                    )
                    .await;
                let (mut intent, lease_expiry) = store
                    .activation_in_transaction(session, &command.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                if intent != command.intent
                    || intent.revision != command.expected_revision
                    || intent.lease.as_ref().map(|lease| lease.holder.as_str())
                        != Some(command.holder.as_str())
                    || lease_expiry.is_none_or(|expires| expires <= now)
                    || !matches!(
                        intent.state,
                        GrantActivationState::BillingAttempting
                            | GrantActivationState::BillingAmbiguous
                            | GrantActivationState::AwaitingProvider
                    )
                    || !intent.capacity_held
                    || intent.provider_receipt.is_some()
                    || intent.cleanup_operation.is_some()
                {
                    return Err(custom(SponsorshipStoreError::StaleGeneration));
                }
                let expected_revision = intent.revision;
                intent.revision = intent.revision.checked_add(1).ok_or_else(|| {
                    custom(SponsorshipStoreError::Malformed(
                        "activation revision exhausted".into(),
                    ))
                })?;
                intent.updated_at = now_string;
                match dependencies {
                    Ok(_) => {
                        intent.state = GrantActivationState::ProviderAttempting;
                        intent.denial = None;
                    }
                    Err(error)
                        if error
                            .get_custom::<SponsorshipStoreError>()
                            .is_some_and(|error| {
                                matches!(
                                    error,
                                    SponsorshipStoreError::GateChanged
                                        | SponsorshipStoreError::LaunchAuthorityChanged
                                        | SponsorshipStoreError::AcceptanceChanged
                                        | SponsorshipStoreError::GrantUnavailable
                                )
                            }) =>
                    {
                        store
                            .release_activation_allocation_in_transaction(session, &intent)
                            .await?;
                        intent.state = GrantActivationState::Denied;
                        intent.denial = Some(GrantActivationDenial::AuthorityInvalidated);
                        intent.capacity_held = false;
                        intent.lease = None;
                    }
                    Err(error) => return Err(error),
                }
                intent
                    .validate()
                    .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
                let retained_expiry = intent.lease.as_ref().and(lease_expiry);
                store
                    .replace_activation_in_transaction(
                        session,
                        expected_revision,
                        &intent,
                        retained_expiry,
                    )
                    .await?;
                Ok(intent)
            }
            .boxed()
        })
        .await
    }

    async fn update_grant_activation(
        &self,
        expected_revision: u64,
        holder: &str,
        intent: &GrantActivationIntent,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        self.operational
            .parse_configured(holder.to_string())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        self.transaction(
            (
                self.clone(),
                expected_revision,
                holder.to_string(),
                intent.clone(),
            ),
            |session, (store, expected, holder, candidate)| {
                async move {
                    let now = store.database_now(session).await?;
                    if matches!(
                        candidate.state,
                        GrantActivationState::AwaitingBilling
                            | GrantActivationState::BillingAttempting
                            | GrantActivationState::AwaitingProvider
                    ) {
                        store
                            .require_gate_in_transaction(session, &candidate.gate_fence)
                            .await?;
                    }
                    let (stored, lease_expiry) = store
                        .activation_in_transaction(session, &candidate.launch_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if stored.revision != *expected
                        || stored.lease.as_ref().map(|lease| lease.holder.as_str())
                            != Some(holder.as_str())
                        || lease_expiry.is_none_or(|expires| expires <= now)
                    {
                        return if stored == *candidate {
                            Ok(stored)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
                    }
                    let next_revision = expected.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "activation revision exhausted".into(),
                        ))
                    })?;
                    if candidate.revision != next_revision
                        || !activation_update_is_legal(&stored, candidate)
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if stored.capacity_held && !candidate.capacity_held {
                        store
                            .release_activation_allocation_in_transaction(session, &stored)
                            .await?;
                    } else if !stored.capacity_held && candidate.capacity_held {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let retained_expiry = candidate.lease.as_ref().and(lease_expiry);
                    store
                        .replace_activation_in_transaction(
                            session,
                            *expected,
                            candidate,
                            retained_expiry,
                        )
                        .await?;
                    Ok(candidate.clone())
                }
                .boxed()
            },
        )
        .await
    }

    async fn commit_grant_activation(
        &self,
        command: CommitGrantActivation,
    ) -> Result<SponsoredGrant, SponsorshipStoreError> {
        command
            .intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .grant
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if command.grant.generation != 1 {
            return Err(SponsorshipStoreError::StaleGeneration);
        }
        if command.grant.state != SponsoredGrantState::Active
            || command.grant.usage != SponsoredUsageSummary::default()
            || command.grant.revoked_at.is_some()
            || command.grant.pending_revocation.is_some()
            || command.grant.created_at != command.grant.updated_at
        {
            return Err(SponsorshipStoreError::Conflict);
        }
        command
            .launch_authority
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .acceptance
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_gate(&command.gate_fence)?;
        validate_audit_event(&command.audit)?;
        let replay_candidate = command.grant.clone();
        let result = self
            .transaction((self.clone(), command), |session, (store, command)| {
                async move {
                    if let Some(document) = store
                        .grants
                        .find_one(doc! { "launch_id": &command.grant.launch_id.0 })
                        .session(&mut *session)
                        .await?
                    {
                        let (grant, _) = read_grant(&store.cipher, &store.operational, document)
                            .map_err(custom)?;
                        return if grant == command.grant {
                            Ok(grant)
                        } else {
                            Err(custom(SponsorshipStoreError::Conflict))
                        };
                    }
                    let now = store.database_now(session).await?;
                    if command.intent.gate_fence != command.gate_fence {
                        return Err(custom(SponsorshipStoreError::GateChanged));
                    }
                    let (policy, credential) = store
                        .require_activation_dependencies_in_transaction(
                            session,
                            &command.intent,
                            &command.launch_authority,
                            &command.acceptance,
                            &database_time_string(now),
                        )
                        .await?;
                    let (stored, lease_expiry) = store
                        .activation_in_transaction(session, &command.intent.launch_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if stored.revision != command.expected_revision
                        || stored.lease.as_ref().map(|lease| lease.holder.as_str())
                            != Some(command.holder.as_str())
                        || lease_expiry.is_none_or(|expires| expires <= now)
                        || stored.state != GrantActivationState::ProviderConfirmed
                        || !stored.capacity_held
                        || command.intent.state != GrantActivationState::Committed
                        || command.intent.capacity_held
                        || command.intent.lease.is_some()
                        || stored.provider_receipt.is_none()
                        || command.intent.provider_receipt != stored.provider_receipt
                        || !activation_lineage_matches(&stored, &command.intent)
                        || stored.denial.is_some()
                        || command.intent.denial.is_some()
                        || stored.cleanup_operation.is_some()
                        || command.intent.cleanup_operation.is_some()
                    {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    if command.intent.revision
                        != command.expected_revision.checked_add(1).ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "activation revision exhausted".into(),
                            ))
                        })?
                        || command.intent.launch_authority_digest != command.launch_authority.digest
                        || command.intent.acceptance_digest != command.acceptance.digest
                        || command.intent.gate_fence != command.gate_fence
                        || command.grant.acceptance != command.acceptance
                        || command.grant.terms_digest != command.acceptance.terms_digest
                        || command.grant.allocation != stored.allocation
                        || command.grant.policy_id != stored.policy_id
                        || command.grant.policy_id != command.launch_authority.policy_id
                        || command.grant.policy_generation != stored.policy_generation
                        || command.grant.policy_generation != policy.generation
                        || command.grant.credential_generation != stored.credential_generation
                        || command.grant.credential_generation != credential.generation
                        || command.grant.tenant_id != command.launch_authority.tenant_id
                        || command.grant.launch_id != command.launch_authority.launch_id
                        || command.grant.contributor != command.launch_authority.contributor
                        || command.grant.trigger_revision_id != command.launch_authority.revision_id
                        || command.grant.trigger_revision_digest
                            != command.launch_authority.revision_digest
                        || command.launch_authority.session_id.as_ref()
                            != Some(&command.grant.session_id)
                        || command.grant.created_at != command.intent.updated_at
                        || command.grant.created_at < stored.updated_at
                        || !deterministic_grant_id_matches(
                            &command.grant.id,
                            &stored.provider_request_id,
                        )
                        || credential.owner != policy.owner
                        || credential.authority.provider != policy.provider_policy.provider
                        || credential.authority.provider_policy_key
                            != policy.provider_policy.provider_policy_key
                    {
                        return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                    }
                    let receipt = stored.provider_receipt.clone().ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "confirmed activation has no provider receipt".into(),
                        ))
                    })?;
                    let database_now = database_time_seconds(now).map_err(custom)?;
                    let issued =
                        timestamp_seconds(&receipt.issued_at, "provider receipt issue time")
                            .map_err(custom)?;
                    let expires = timestamp_seconds(&receipt.expires_at, "provider receipt expiry")
                        .map_err(custom)?;
                    let created =
                        timestamp_seconds(&command.grant.created_at, "grant creation time")
                            .map_err(custom)?;
                    let policy_expires =
                        timestamp_seconds(&policy.expires_at, "policy expiry").map_err(custom)?;
                    if issued > database_now
                        || expires <= database_now
                        || expires > policy_expires
                        || created > database_now
                        || created < issued
                        || created >= expires
                    {
                        return Err(custom(SponsorshipStoreError::GrantUnavailable));
                    }
                    require_receipt_matches_grant(&store.operational, &receipt, &command.grant)
                        .map_err(custom)?;
                    if store
                        .grants
                        .find_one(doc! { "launch_id": &command.grant.launch_id.0 })
                        .session(&mut *session)
                        .await?
                        .is_some()
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    store
                        .grants
                        .insert_one(
                            grant_doc(
                                &store.cipher,
                                &store.operational,
                                &command.grant,
                                &receipt,
                                &policy.owner.controller().0,
                            )
                            .map_err(custom)?,
                        )
                        .session(&mut *session)
                        .await?;
                    store
                        .replace_activation_in_transaction(
                            session,
                            command.expected_revision,
                            &command.intent,
                            None,
                        )
                        .await?;
                    store.audit_in_transaction(session, &command.audit).await?;
                    Ok(command.grant.clone())
                }
                .boxed()
            })
            .await;
        if matches!(
            &result,
            Err(SponsorshipStoreError::Conflict | SponsorshipStoreError::StaleGeneration)
        ) {
            let stored = self
                .grants
                .find_one(doc! { "launch_id": &replay_candidate.launch_id.0 })
                .await
                .map_err(|error| {
                    stored_read_failure("authenticate committed sponsored grant replay", error)
                })?
                .map(|document| {
                    read_grant(&self.cipher, &self.operational, document).map(|(grant, _)| grant)
                })
                .transpose()?;
            if let Some(stored) = stored {
                if stored == replay_candidate {
                    return Ok(stored);
                }
            }
        }
        result
    }

    async fn complete_grant_activation_cleanup(
        &self,
        command: CompleteGrantActivationCleanup,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        command
            .intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .operation
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&command.audit)?;
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let (stored, lease_expiry) = store
                    .activation_in_transaction(session, &command.intent.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                if stored == command.intent {
                    return Ok(stored);
                }
                let stored_operation = stored
                    .cleanup_operation
                    .as_ref()
                    .ok_or_else(|| custom(SponsorshipStoreError::StaleGeneration))?;
                let global_credential_retirement = if command.global_credential_retirement {
                    let (policy, _) = store
                        .policy_in_transaction(session, &stored.policy_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::StaleGeneration))?;
                    let credential = store
                        .credential_record_in_transaction(session, &policy.credential_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::StaleGeneration))?;
                    credential_retirement_covers_activation(&stored, &policy, &credential)
                } else {
                    false
                };
                let cleanup_completion_is_authorized = matches!(
                    stored_operation.phase,
                    ProviderOperationPhase::Attempting | ProviderOperationPhase::Ambiguous
                ) || (stored_operation.phase
                    == ProviderOperationPhase::Rejected
                    && global_credential_retirement);
                if stored.revision != command.expected_revision
                    || stored.lease.as_ref().map(|lease| lease.holder.as_str())
                        != Some(command.holder.as_str())
                    || lease_expiry.is_none_or(|expires| expires <= now)
                    || stored.state != GrantActivationState::CleanupPending
                    || !stored.capacity_held
                    || stored.provider_receipt != command.intent.provider_receipt
                    || !activation_lineage_matches(&stored, &command.intent)
                    || command.intent.state != GrantActivationState::Cleaned
                    || command.intent.denial.is_some()
                    || command.intent.capacity_held
                    || command.intent.lease.is_some()
                    || command.intent.cleanup_operation.is_some()
                    || (command.global_credential_retirement && !global_credential_retirement)
                    || !cleanup_completion_is_authorized
                    || !operation_lineage_matches(stored_operation, &command.operation)
                    || command.intent.updated_at < stored.updated_at
                    || command.audit.occurred_at != command.intent.updated_at
                    || command.intent.revision
                        != command.expected_revision.checked_add(1).ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "activation revision exhausted".into(),
                            ))
                        })?
                {
                    return Err(custom(SponsorshipStoreError::StaleGeneration));
                }
                store
                    .release_activation_allocation_in_transaction(session, &stored)
                    .await?;
                store
                    .replace_activation_in_transaction(
                        session,
                        command.expected_revision,
                        &command.intent,
                        None,
                    )
                    .await?;
                store.audit_in_transaction(session, &command.audit).await?;
                Ok(command.intent.clone())
            }
            .boxed()
        })
        .await
    }

    async fn complete_invalidated_grant_activation(
        &self,
        command: CompleteInvalidatedGrantActivation,
    ) -> Result<GrantActivationIntent, SponsorshipStoreError> {
        command
            .intent
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&command.audit)?;
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let (stored, lease_expiry) = store
                    .activation_in_transaction(session, &command.intent.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                if stored == command.intent {
                    return Ok(stored);
                }
                if stored.revision != command.expected_revision
                    || stored.lease.as_ref().map(|lease| lease.holder.as_str())
                        != Some(command.holder.as_str())
                    || lease_expiry.is_none_or(|expires| expires <= now)
                    || !stored.capacity_held
                    || !matches!(
                        stored.state,
                        GrantActivationState::BillingAttempting
                            | GrantActivationState::BillingAmbiguous
                            | GrantActivationState::ProviderAttempting
                            | GrantActivationState::ProviderAmbiguous
                    )
                    || !activation_lineage_matches(&stored, &command.intent)
                    || stored.provider_receipt.is_some()
                    || command.intent.provider_receipt.is_some()
                    || stored.cleanup_operation.is_some()
                    || command.intent.cleanup_operation.is_some()
                    || stored.denial.is_some()
                    || command.intent.state != GrantActivationState::Denied
                    || command.intent.denial != Some(GrantActivationDenial::AuthorityInvalidated)
                    || command.intent.capacity_held
                    || command.intent.lease.is_some()
                    || command.intent.updated_at < stored.updated_at
                    || command.audit.occurred_at != command.intent.updated_at
                    || command.intent.revision
                        != command.expected_revision.checked_add(1).ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "activation revision exhausted".into(),
                            ))
                        })?
                {
                    return Err(custom(SponsorshipStoreError::StaleGeneration));
                }
                store
                    .release_activation_allocation_in_transaction(session, &stored)
                    .await?;
                store
                    .replace_activation_in_transaction(
                        session,
                        command.expected_revision,
                        &command.intent,
                        None,
                    )
                    .await?;
                store.audit_in_transaction(session, &command.audit).await?;
                Ok(command.intent.clone())
            }
            .boxed()
        })
        .await
    }

    async fn provider_grant_receipt(
        &self,
        grant_id: &SponsoredGrantId,
    ) -> Result<Option<ProviderGrantReceipt>, SponsorshipStoreError> {
        self.grants
            .find_one(doc! { "_id": &grant_id.0 })
            .await
            .map_err(|error| stored_read_failure("read encrypted provider grant receipt", error))?
            .map(|document| {
                read_grant(&self.cipher, &self.operational, document).map(|(_, receipt)| receipt)
            })
            .transpose()
    }

    async fn grant(
        &self,
        id: &SponsoredGrantId,
    ) -> Result<Option<SponsoredGrant>, SponsorshipStoreError> {
        self.grants
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored grant", error))?
            .as_ref()
            .map(|document| read_grant_record(&self.operational, document))
            .transpose()
    }

    async fn grants_for_contributor(
        &self,
        contributor: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredGrant>, SponsorshipStoreError> {
        validate_subject_query(contributor)?;
        let filter = newest_page_filter(doc! { "contributor": &contributor.0 }, page)?;
        let documents = self
            .grants
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(page_query_limit(page))
            .await
            .map_err(|error| stored_read_failure("list contributor sponsored grants", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| stored_read_failure("list contributor sponsored grants", error))?;
        let mut grants = Vec::with_capacity(documents.len());
        for document in documents {
            if document.contributor != contributor.0 {
                return Err(SponsorshipStoreError::Malformed(
                    "contributor grant index crossed its subject boundary".into(),
                ));
            }
            grants.push(read_grant_record(&self.operational, &document)?);
        }
        Ok(finish_newest_page(grants, page, |grant| {
            (&grant.created_at, &grant.id.0)
        }))
    }

    async fn grants_for_sponsor(
        &self,
        sponsor: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<SponsoredGrant>, SponsorshipStoreError> {
        validate_subject_query(sponsor)?;
        let filter = newest_page_filter(doc! { "sponsor_subject": &sponsor.0 }, page)?;
        let documents = self
            .grants
            .find(filter)
            .sort(doc! { "created_at": -1, "_id": -1 })
            .limit(page_query_limit(page))
            .await
            .map_err(|error| stored_read_failure("list sponsor-owned grants", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| stored_read_failure("list sponsor-owned grants", error))?;
        let mut grants = Vec::with_capacity(documents.len());
        for document in documents {
            if document.sponsor_subject != sponsor.0 {
                return Err(SponsorshipStoreError::Malformed(
                    "sponsor grant index crossed its subject boundary".into(),
                ));
            }
            let grant = read_grant_record(&self.operational, &document)?;
            let policy = self.policy(&grant.policy_id).await?.ok_or_else(|| {
                SponsorshipStoreError::Malformed(
                    "sponsor grant index references a missing policy".into(),
                )
            })?;
            require_grant_policy_bindings(&grant, &policy)?;
            if policy.owner.controller() != sponsor {
                return Err(SponsorshipStoreError::Malformed(
                    "sponsor grant index does not match its policy owner".into(),
                ));
            }
            grants.push(grant);
        }
        Ok(finish_newest_page(grants, page, |grant| {
            (&grant.created_at, &grant.id.0)
        }))
    }

    async fn grant_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrant>, SponsorshipStoreError> {
        self.grants
            .find_one(doc! { "launch_id": &launch_id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored grant for Launch", error))?
            .as_ref()
            .map(|document| read_grant_record(&self.operational, document))
            .transpose()
    }

    async fn usage_reservation(
        &self,
        reservation_id: &SponsoredUsageReservationId,
    ) -> Result<Option<SponsoredUsageReservation>, SponsorshipStoreError> {
        self.reservations
            .find_one(doc! { "_id": &reservation_id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored usage reservation", error))?
            .map(|document| {
                read_reservation(&self.operational, document).map(|(reservation, _, _)| reservation)
            })
            .transpose()
    }

    async fn usage_reservation_for_request(
        &self,
        request_identity: &SponsorshipOperationalPseudonymSet,
    ) -> Result<Option<SponsoredUsageReservation>, SponsorshipStoreError> {
        let aliases = request_identity.encoded_candidates();
        validate_operational_aliases(&self.operational, &aliases, "usage request candidate")?;
        self.transaction(
            (self.clone(), request_identity.clone()),
            |session, (store, candidates)| {
                async move {
                    let Some((reservation, _, _, stored_aliases)) = store
                        .reservation_for_request_in_transaction(session, candidates)
                        .await?
                    else {
                        return Ok(None);
                    };
                    let aliases = candidates.encoded_candidates();
                    if stored_aliases != aliases {
                        let updated = store
                            .reservations
                            .update_one(
                                doc! { "_id": &reservation.id.0 },
                                doc! { "$set": { "request_aliases": aliases } },
                            )
                            .session(&mut *session)
                            .await?;
                        if updated.matched_count != 1 {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                    }
                    Ok(Some(reservation))
                }
                .boxed()
            },
        )
        .await
    }

    async fn replace_grant(
        &self,
        expected_generation: u64,
        grant: &SponsoredGrant,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredGrant, SponsorshipStoreError> {
        grant
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        require_successor_generation(expected_generation, grant.generation, "grant")?;
        self.transaction(
            (self.clone(), expected_generation, grant.clone(), audit),
            |session, (store, expected, candidate, audit)| {
                async move {
                    let (stored, stored_document) = store
                        .grant_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if stored.generation != *expected {
                        return if stored == *candidate {
                            Ok(stored)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
                    }
                    if stored == *candidate {
                        return Ok(stored);
                    }
                    if !grant_update_is_legal(&stored, candidate) {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    store
                        .replace_grant_in_transaction(
                            session,
                            *expected,
                            candidate,
                            &stored_document.sponsor_subject,
                            stored_document.unresolved_usage,
                            stored_document.provider_receipt,
                            stored_document.encryption_key_id,
                        )
                        .await?;
                    store.audit_in_transaction(session, audit).await?;
                    Ok(candidate.clone())
                }
                .boxed()
            },
        )
        .await
    }

    async fn release_grant_capacity(
        &self,
        expected_generation: u64,
        grant: &SponsoredGrant,
        reason: GrantCapacityRelease,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredGrant, SponsorshipStoreError> {
        grant
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        require_successor_generation(expected_generation, grant.generation, "grant")?;
        self.transaction(
            (
                self.clone(),
                expected_generation,
                grant.clone(),
                reason,
                audit,
            ),
            |session, (store, expected, candidate, reason, audit)| {
                async move {
                    let (policy, _) = store
                        .policy_in_transaction(session, &candidate.policy_id)
                        .await?
                        .ok_or_else(|| {
                            custom(SponsorshipStoreError::Malformed(
                                "stored grant references a missing sponsorship policy".into(),
                            ))
                        })?;
                    let credential = if matches!(
                        *reason,
                        GrantCapacityRelease::Revoked | GrantCapacityRelease::Closed
                    ) {
                        Some(
                            store
                                .credential_record_in_transaction(session, &policy.credential_id)
                                .await?
                                .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?,
                        )
                    } else {
                        None
                    };
                    let (stored, stored_document) = store
                        .grant_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    require_grant_policy_bindings(&stored, &policy).map_err(custom)?;
                    if stored == *candidate {
                        return Ok(stored);
                    }
                    if stored.generation != *expected {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    let expected_state = match reason {
                        GrantCapacityRelease::Expired => SponsoredGrantState::Expired,
                        GrantCapacityRelease::Exhausted => SponsoredGrantState::Exhausted,
                        GrantCapacityRelease::Revoked => SponsoredGrantState::Revoked,
                        GrantCapacityRelease::Closed => SponsoredGrantState::Closed,
                    };
                    if grant_capacity_is_released(stored.state)
                        || candidate.state != expected_state
                        || !grant_lineage_matches(&stored, candidate)
                        || stored.usage != candidate.usage
                        || stored.allocation != candidate.allocation
                        || candidate.pending_revocation.is_some()
                        || candidate.updated_at < stored.updated_at
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    match *reason {
                        GrantCapacityRelease::Expired => {
                            let now = database_time_seconds(store.database_now(session).await?)
                                .map_err(custom)?;
                            let expiry = timestamp_seconds(
                                &stored.authority.expires_at,
                                "provider grant expiry",
                            )
                            .map_err(custom)?;
                            if now < expiry {
                                return Err(custom(SponsorshipStoreError::GrantUnavailable));
                            }
                        }
                        GrantCapacityRelease::Exhausted => {
                            if !candidate.usage.reaches_any_limit(&candidate.allocation) {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                        }
                        GrantCapacityRelease::Revoked | GrantCapacityRelease::Closed => {
                            let expected_kind = if *reason == GrantCapacityRelease::Revoked {
                                ProviderOperationKind::RevokeGrant
                            } else {
                                ProviderOperationKind::CloseGrant
                            };
                            let operation = stored.pending_revocation.as_ref();
                            if operation.is_some_and(|operation| operation.kind != expected_kind) {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            let credential = credential
                                .as_ref()
                                .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                            let confirmed_revocation = credential
                                .last_completed_operation
                                .as_ref()
                                .filter(|operation| {
                                    operation.kind == ProviderOperationKind::RevokeCredential
                                        && operation.outcome
                                            == CompletedProviderOperationOutcome::Confirmed
                                });
                            let globally_retired = credential.state
                                == SponsoredCredentialState::Revoked
                                && credential.material_state
                                    == SponsoredCredentialMaterialState::Retired
                                && credential.revoked_at.is_some()
                                && confirmed_revocation.is_some()
                                && policy.id == stored.policy_id
                                && policy.tenant_id == stored.tenant_id
                                && policy.generation >= stored.policy_generation
                                && credential.generation > stored.credential_generation
                                && credential.owner == policy.owner
                                && credential.authority.provider == policy.provider_policy.provider
                                && credential.authority.provider_policy_key
                                    == policy.provider_policy.provider_policy_key;
                            let attempted_grant_revocation = stored.state
                                == SponsoredGrantState::RevocationPending
                                && operation.is_some_and(|operation| {
                                    matches!(
                                        operation.phase,
                                        ProviderOperationPhase::Attempting
                                            | ProviderOperationPhase::Ambiguous
                                    )
                                });
                            if !globally_retired && !attempted_grant_revocation {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            let terminal_at = candidate
                                .revoked_at
                                .as_ref()
                                .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                            if terminal_at < &stored.created_at
                                || terminal_at < &stored.authority.issued_at
                                || candidate.updated_at < *terminal_at
                                || operation.is_some_and(|operation| {
                                    terminal_at < &operation.requested_at
                                        || operation
                                            .attempted_at
                                            .as_ref()
                                            .is_some_and(|attempted_at| terminal_at < attempted_at)
                                })
                                || confirmed_revocation.is_some_and(|completion| {
                                    terminal_at < &completion.completed_at
                                })
                            {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                        }
                    }
                    store
                        .release_terminal_grant_capacity_in_transaction(
                            session,
                            &stored,
                            &stored_document.unresolved_usage,
                        )
                        .await?;
                    store
                        .replace_grant_in_transaction(
                            session,
                            *expected,
                            candidate,
                            &stored_document.sponsor_subject,
                            stored_document.unresolved_usage,
                            stored_document.provider_receipt,
                            stored_document.encryption_key_id,
                        )
                        .await?;
                    store.audit_in_transaction(session, audit).await?;
                    Ok(candidate.clone())
                }
                .boxed()
            },
        )
        .await
    }

    async fn reserve_usage(
        &self,
        command: ReserveSponsoredUsage,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredUsageReservationOutcome, SponsorshipStoreError> {
        let request_fingerprint = command
            .request
            .fingerprint()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .price_quote
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if let Some(dispatch) = command.model_dispatch.as_ref() {
            dispatch
                .validate()
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        }
        command
            .launch_authority
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let request_aliases = command.request.request_identity.encoded_candidates();
        validate_operational_aliases(
            &self.operational,
            &request_aliases,
            "usage request candidate",
        )?;
        validate_gate(&command.gate_fence)?;
        validate_audit_event(&audit)?;
        let replay_identity = command.request.request_identity.clone();
        let replay_fingerprint = command.request.idempotency_fingerprint.clone();
        let result = self
            .transaction(
                (self.clone(), command, request_fingerprint, audit),
                |session, (store, command, request_fingerprint, audit)| {
                    async move {
                        if let Some((existing, _, _, stored_aliases)) = store
                            .reservation_for_request_in_transaction(
                                session,
                                &command.request.request_identity,
                            )
                            .await?
                        {
                            if existing.idempotency_fingerprint
                                != command.request.idempotency_fingerprint
                            {
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            let request_aliases =
                                command.request.request_identity.encoded_candidates();
                            if stored_aliases != request_aliases {
                                let updated = store
                                    .reservations
                                    .update_one(
                                        doc! { "_id": &existing.id.0 },
                                        doc! { "$set": { "request_aliases": request_aliases } },
                                    )
                                    .session(&mut *session)
                                    .await?;
                                if updated.matched_count != 1 {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                            }
                            return Ok(SponsoredUsageReservationOutcome {
                                disposition: ReservationDisposition::Existing,
                                reservation: existing,
                            });
                        }
                        let now = store.database_now(session).await?;
                        let now_string = database_time_string(now);
                        store
                            .require_gate_in_transaction(session, &command.gate_fence)
                            .await?;
                        if command.request.price_quote_digest != command.price_quote.digest
                            || command.request.maximum != command.price_quote.maximum
                        {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                        let launch = store
                            .launch_in_transaction(session, &command.launch_authority.launch_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
                        let policy_id = launch_sponsorship_policy_id(&launch).map_err(custom)?;
                        let acceptance = store
                            .acceptance_in_transaction(session, &launch.id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::AcceptanceChanged))?;
                        let (policy, _) =
                            store
                                .policy_in_transaction(session, &policy_id)
                                .await?
                                .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                        let credential = store
                            .credential_record_in_transaction(session, &policy.credential_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                        let (grant, grant_document) = store
                            .grant_in_transaction(session, &command.request.grant_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                        require_grant_policy_bindings(&grant, &policy).map_err(custom)?;
                        let current_launch = SponsoredLaunchAuthority::from_launch(
                            &launch, &policy,
                        )
                        .map_err(|_| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
                        if current_launch != command.launch_authority
                            || !matches!(
                                current_launch.state,
                                SponsoredLaunchState::Active | SponsoredLaunchState::Recoverable
                            )
                        {
                            return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                        }
                        let terms_digest = SponsoredGrantTerms::digest_from_policy(&policy)
                            .map_err(|error| {
                                custom(SponsorshipStoreError::Malformed(error.to_string()))
                            })?;
                        if command
                            .gate_fence
                            .runtime_limits
                            .require_policy_at(&policy, &now_string)
                            .is_err()
                            || policy.provider_policy.provider_policy_key
                                != command.gate_fence.trusted_provider_policy_key
                            || !grant.is_active_at(&now_string)
                            || !policy.is_available_at(&now_string)
                            || credential.state != SponsoredCredentialState::Active
                            || credential.material_state
                                != SponsoredCredentialMaterialState::Retained
                            || credential.generation < grant.credential_generation
                            || credential.owner != policy.owner
                            || credential.authority.provider != policy.provider_policy.provider
                            || credential.authority.provider_policy_key
                                != policy.provider_policy.provider_policy_key
                            || grant.policy_generation != command.policy_generation
                            || policy.generation != command.policy_generation
                            || grant.credential_generation != command.credential_generation
                            || acceptance != grant.acceptance
                            || acceptance.policy_generation != policy.generation
                            || acceptance.terms_digest != terms_digest
                            || grant.terms_digest != acceptance.terms_digest
                            || acceptance.launch_id != current_launch.launch_id
                            || acceptance.contributor != current_launch.contributor
                            || acceptance.revision_id != current_launch.revision_id
                            || acceptance.revision_digest != current_launch.revision_digest
                            || acceptance.policy_id != current_launch.policy_id
                            || acceptance.launch_snapshot_digest
                                != current_launch.launch_snapshot_digest
                            || grant.contributor != command.request.requester
                            || grant.capability.fingerprint
                                != command.request.capability_fingerprint
                            || grant.tenant_id != command.request.tenant_id
                            || grant.trigger_revision_id != command.request.trigger_revision_id
                            || grant.trigger_revision_digest
                                != command.request.trigger_revision_digest
                            || grant.launch_id != command.request.launch_id
                            || grant.session_id != command.request.session_id
                            || grant.launch_id != current_launch.launch_id
                            || grant.tenant_id != current_launch.tenant_id
                            || grant.policy_id != current_launch.policy_id
                            || grant.trigger_revision_id != current_launch.revision_id
                            || grant.trigger_revision_digest != current_launch.revision_digest
                            || current_launch.session_id.as_ref() != Some(&grant.session_id)
                            || current_launch.contributor != grant.contributor
                            || command.request.provider != policy.provider_policy.provider
                            || !policy
                                .provider_policy
                                .allowed_models
                                .contains(&command.request.model)
                            || !policy
                                .provider_policy
                                .allowed_resources
                                .contains(&command.request.resource)
                        {
                            return Err(custom(SponsorshipStoreError::GrantUnavailable));
                        }
                        let launch_projected =
                            summary_add(&grant.usage, &grant_document.unresolved_usage)
                                .and_then(|usage| summary_add(&usage, &command.request.maximum))
                                .map_err(custom)?;
                        if !launch_projected.quota.fits_within(grant.allocation.quota) {
                            return Err(custom(SponsorshipStoreError::LaunchQuotaExceeded));
                        }
                        if launch_projected.spend_micros > grant.allocation.spend_micros {
                            return Err(custom(SponsorshipStoreError::LaunchBudgetExceeded));
                        }

                        store
                            .advance_policy_rate_fence_in_transaction(session, &policy.id)
                            .await?;
                        let window_start = DateTime::from_millis(
                            now.timestamp_millis()
                                .checked_sub(RATE_WINDOW_MILLIS)
                                .ok_or_else(|| {
                                    custom(SponsorshipStoreError::Malformed(
                                        "sponsorship rate window underflowed".into(),
                                    ))
                                })?,
                        );
                        let recent = store
                            .reservations
                            .count_documents(doc! {
                                "policy_id": &policy.id.0,
                                "rate_observed_at": { "$gt": window_start },
                            })
                            .session(&mut *session)
                            .await?;
                        if recent >= u64::from(policy.max_requests_per_minute) {
                            return Err(custom(SponsorshipStoreError::RateExceeded));
                        }

                        let reservation = SponsoredUsageReservation {
                            id: command.request.reservation_id.clone(),
                            idempotency_fingerprint: command
                                .request
                                .idempotency_fingerprint
                                .clone(),
                            request_fingerprint: request_fingerprint.clone(),
                            grant_id: grant.id.clone(),
                            price_quote: command.price_quote.clone(),
                            reserved: command.request.maximum.clone(),
                            state: SponsoredUsageState::Reserved,
                            dispatch_revision: 0,
                            dispatch_lease: None,
                            created_at: command.now.clone(),
                            updated_at: command.now.clone(),
                            actual: None,
                        };
                        reservation.validate().map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })?;
                        let unresolved_usage =
                            summary_add(&grant_document.unresolved_usage, &reservation.reserved)
                                .map_err(custom)?;
                        store
                            .replace_grant_in_transaction(
                                session,
                                grant.generation,
                                &grant,
                                &grant_document.sponsor_subject,
                                unresolved_usage,
                                grant_document.provider_receipt,
                                grant_document.encryption_key_id,
                            )
                            .await?;
                        let reservation_document = match command.model_dispatch.as_ref() {
                            Some(dispatch) => model_reservation_doc(
                                &store.cipher,
                                &store.operational,
                                &reservation,
                                &command.request.request_identity.encoded_candidates(),
                                &policy.id,
                                now,
                                &policy.credential_id,
                                dispatch,
                            ),
                            None => reservation_doc(
                                &store.operational,
                                &reservation,
                                &command.request.request_identity.encoded_candidates(),
                                &policy.id,
                                now,
                            ),
                        }
                        .map_err(custom)?;
                        store
                            .reservations
                            .insert_one(reservation_document)
                            .session(&mut *session)
                            .await?;
                        store.audit_in_transaction(session, audit).await?;
                        Ok(SponsoredUsageReservationOutcome {
                            disposition: ReservationDisposition::Created,
                            reservation,
                        })
                    }
                    .boxed()
                },
            )
            .await;
        if result == Err(SponsorshipStoreError::Conflict) {
            if let Some(existing) = self.usage_reservation_for_request(&replay_identity).await? {
                if existing.idempotency_fingerprint == replay_fingerprint {
                    return Ok(SponsoredUsageReservationOutcome {
                        disposition: ReservationDisposition::Existing,
                        reservation: existing,
                    });
                }
            }
        }
        result
    }

    async fn claim_usage_dispatch(
        &self,
        command: ClaimSponsoredUsageDispatch,
    ) -> Result<SponsoredUsageDispatchClaim, SponsorshipStoreError> {
        SponsoredUsageReservationId::parse(command.reservation_id.0.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        if !is_digest(&command.request_fingerprint) {
            return Err(SponsorshipStoreError::Malformed(
                "usage dispatch request fingerprint is invalid".into(),
            ));
        }
        command
            .capability
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .gate_fence
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&command.audit)?;
        if command.audit.action != SponsoredAuditAction::UsageDispatchClaimed
            || command.audit.outcome_code != "attempting"
            || command.audit.stable_request_id_digest.is_none()
        {
            return Err(SponsorshipStoreError::Malformed(
                "usage dispatch claim audit is invalid".into(),
            ));
        }
        if uuid::Uuid::parse_str(&command.holder)
            .ok()
            .is_none_or(|holder| holder.to_string() != command.holder)
            || !matches!(
                command.expected_state,
                SponsoredUsageState::Reserved
                    | SponsoredUsageState::Attempting
                    | SponsoredUsageState::Ambiguous
            )
        {
            return Err(SponsorshipStoreError::Malformed(
                "usage dispatch claim identity or state is invalid".into(),
            ));
        }

        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let now_string = database_time_string(now);
                let Some((current, policy_id, _rate_observed_at, _request_aliases, document)) =
                    store
                        .reservation_in_transaction(session, &command.reservation_id)
                        .await?
                else {
                    return Err(custom(SponsorshipStoreError::Conflict));
                };
                if current.request_fingerprint != command.request_fingerprint
                    || current.grant_id != command.capability.grant_id
                    || document.model_dispatch_payload.is_none()
                {
                    return Err(custom(SponsorshipStoreError::Conflict));
                }
                let lease_is_live = current
                    .dispatch_lease
                    .as_ref()
                    .map(|lease| {
                        timestamp_date_time(&lease.expires_at, "usage dispatch lease expiry")
                            .map(|expires| expires > now)
                    })
                    .transpose()
                    .map_err(custom)?
                    .unwrap_or(false);
                if current.state != command.expected_state
                    || current.dispatch_revision != command.expected_dispatch_revision
                    || matches!(
                        current.state,
                        SponsoredUsageState::Settled | SponsoredUsageState::Released
                    )
                    || lease_is_live
                {
                    return Ok(SponsoredUsageDispatchClaim {
                        acquired: false,
                        reservation: current,
                    });
                }
                if !matches!(
                    current.state,
                    SponsoredUsageState::Reserved
                        | SponsoredUsageState::Attempting
                        | SponsoredUsageState::Ambiguous
                ) {
                    return Err(custom(SponsorshipStoreError::Conflict));
                }

                store
                    .require_gate_in_transaction(session, &command.gate_fence)
                    .await?;
                let (grant, grant_document) = store
                    .grant_authority_in_transaction(session, &current.grant_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                let (policy, _) = store
                    .policy_in_transaction(session, &policy_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                require_grant_policy_bindings(&grant, &policy).map_err(custom)?;
                if document.credential_id.as_deref() != Some(policy.credential_id.0.as_str()) {
                    return Err(custom(SponsorshipStoreError::Malformed(
                        "model usage credential index does not match its policy".into(),
                    )));
                }
                let credential = store
                    .credential_record_in_transaction(session, &policy.credential_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                let launch = store
                    .launch_in_transaction(session, &grant.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
                let launch_authority = SponsoredLaunchAuthority::from_launch(&launch, &policy)
                    .map_err(|_| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
                let acceptance = store
                    .acceptance_in_transaction(session, &grant.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::AcceptanceChanged))?;
                let terms_digest = SponsoredGrantTerms::digest_from_policy(&policy)
                    .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
                if grant_document.sponsor_subject != grant.sponsor_subject.0
                    || !grant.is_active_at(&now_string)
                    || grant.capability != command.capability
                    || !policy.is_available_at(&now_string)
                    || command
                        .gate_fence
                        .runtime_limits
                        .require_policy_at(&policy, &now_string)
                        .is_err()
                    || policy.provider_policy.provider_policy_key
                        != command.gate_fence.trusted_provider_policy_key
                    || grant.policy_generation != policy.generation
                    || !credential.permits_retained_model_dispatch()
                    || credential.generation < grant.credential_generation
                    || credential.owner != policy.owner
                    || credential.authority.provider != policy.provider_policy.provider
                    || credential.authority.provider_policy_key
                        != policy.provider_policy.provider_policy_key
                    || !matches!(
                        launch_authority.state,
                        SponsoredLaunchState::Active | SponsoredLaunchState::Recoverable
                    )
                    || launch_authority.launch_id != grant.launch_id
                    || launch_authority.tenant_id != grant.tenant_id
                    || launch_authority.policy_id != grant.policy_id
                    || launch_authority.contributor != grant.contributor
                    || launch_authority.session_id.as_ref() != Some(&grant.session_id)
                    || acceptance != grant.acceptance
                    || acceptance.policy_generation != policy.generation
                    || acceptance.terms_digest != terms_digest
                    || current.price_quote.provider != policy.provider_policy.provider
                    || !policy
                        .provider_policy
                        .allowed_models
                        .contains(&current.price_quote.model)
                    || !policy
                        .provider_policy
                        .allowed_resources
                        .contains(&current.price_quote.resource)
                {
                    return Err(custom(SponsorshipStoreError::GrantUnavailable));
                }
                if command.audit.policy_id.as_ref() != Some(&policy.id)
                    || command.audit.grant_id.as_ref() != Some(&grant.id)
                {
                    return Err(custom(SponsorshipStoreError::Malformed(
                        "usage dispatch claim audit target is invalid".into(),
                    )));
                }

                let mut claimed = current.clone();
                claimed.dispatch_revision =
                    claimed.dispatch_revision.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "usage dispatch revision exhausted".into(),
                        ))
                    })?;
                claimed.state = SponsoredUsageState::Attempting;
                claimed.dispatch_lease =
                    Some(usage_dispatch_lease(command.holder.clone(), now).map_err(custom)?);
                claimed.updated_at = now_string;
                claimed
                    .validate()
                    .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
                let replacement =
                    replace_reservation_projection(document, &claimed).map_err(custom)?;
                let mut filter = doc! {
                    "_id": &current.id.0,
                    "state": usage_state_code(current.state),
                    "updated_at": &current.updated_at,
                    "reservation.dispatch_revision": signed(
                        current.dispatch_revision,
                        "usage dispatch revision",
                    )
                    .map_err(custom)?,
                };
                match current.dispatch_lease.as_ref() {
                    Some(lease) => {
                        filter.insert("reservation.dispatch_lease.holder", lease.holder.clone());
                        filter.insert(
                            "reservation.dispatch_lease.expires_at",
                            lease.expires_at.clone(),
                        );
                    }
                    None => {
                        filter.insert("reservation.dispatch_lease", doc! { "$exists": false });
                    }
                }
                let result = store
                    .reservations
                    .replace_one(filter, replacement)
                    .session(&mut *session)
                    .await?;
                if result.matched_count != 1 {
                    return Err(custom(SponsorshipStoreError::StaleGeneration));
                }
                let mut audit = command.audit.clone();
                audit.occurred_at = claimed.updated_at.clone();
                validate_audit_event(&audit).map_err(custom)?;
                store.audit_in_transaction(session, &audit).await?;
                Ok(SponsoredUsageDispatchClaim {
                    acquired: true,
                    reservation: claimed,
                })
            }
            .boxed()
        })
        .await
    }

    async fn reconcile_usage(
        &self,
        command: ReconcileSponsoredUsage,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredUsageReservation, SponsorshipStoreError> {
        validate_audit_event(&audit)?;
        if let Some(SponsoredUsageReconciliationFence::DispatchClaim { revision, holder }) =
            command.dispatch_fence.as_ref()
        {
            if *revision == 0
                || Uuid::parse_str(holder)
                    .ok()
                    .is_none_or(|parsed| parsed.to_string() != *holder)
            {
                return Err(SponsorshipStoreError::Malformed(
                    "usage reconciliation dispatch claim is invalid".into(),
                ));
            }
        }
        self.transaction(
            (self.clone(), command, audit),
            |session, (store, command, audit)| {
                async move {
                    let (current, _policy_id, _rate_observed_at, _request_aliases, document) =
                        store
                            .reservation_in_transaction(session, &command.reservation_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if current.request_fingerprint != command.request_fingerprint {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if current.state == command.outcome
                        && current.actual == command.actual
                        && command.actual_quota
                            == current.actual.as_ref().map(|actual| actual.quota)
                    {
                        let stored_response =
                            read_model_response(&store.cipher, &document).map_err(custom)?;
                        if stored_response.as_ref() == command.model_response.as_ref() {
                            return Ok(current);
                        }
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    let has_model_request = document.model_dispatch_payload.is_some();
                    if has_model_request != command.dispatch_fence.is_some() {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if has_model_request
                        && command.outcome == SponsoredUsageState::Settled
                        && command.model_response.is_none()
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if !has_model_request && command.model_response.is_some() {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if let Some(response) = command.model_response.as_ref() {
                        response.validate().map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })?;
                        if Some(response.actual_quota()) != command.actual_quota {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                    }
                    match command.dispatch_fence.as_ref() {
                        Some(SponsoredUsageReconciliationFence::DispatchClaim {
                            revision,
                            holder,
                        }) => {
                            if current.state != SponsoredUsageState::Attempting
                                || current.dispatch_revision != *revision
                                || current
                                    .dispatch_lease
                                    .as_ref()
                                    .is_none_or(|lease| lease.holder != *holder)
                            {
                                return Err(custom(SponsorshipStoreError::StaleGeneration));
                            }
                        }
                        Some(SponsoredUsageReconciliationFence::ProviderObservation {
                            state,
                            revision,
                        }) => {
                            if current.state != *state
                                || current.dispatch_revision != *revision
                                || !matches!(
                                    command.outcome,
                                    SponsoredUsageState::Settled | SponsoredUsageState::Released
                                )
                            {
                                return Err(custom(SponsorshipStoreError::StaleGeneration));
                            }
                        }
                        Some(SponsoredUsageReconciliationFence::NeverDispatched)
                            if command.outcome != SponsoredUsageState::Released
                                || current.state != SponsoredUsageState::Reserved
                                || current.dispatch_revision != 0
                                || current.dispatch_lease.is_some() =>
                        {
                            return Err(custom(SponsorshipStoreError::StaleGeneration));
                        }
                        Some(SponsoredUsageReconciliationFence::NeverDispatched) | None => {}
                    }
                    if has_model_request
                        && command.outcome == SponsoredUsageState::Ambiguous
                        && !matches!(
                            command.dispatch_fence,
                            Some(SponsoredUsageReconciliationFence::DispatchClaim { .. })
                        )
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if matches!(
                        current.state,
                        SponsoredUsageState::Settled | SponsoredUsageState::Released
                    ) || !matches!(
                        command.outcome,
                        SponsoredUsageState::Settled
                            | SponsoredUsageState::Released
                            | SponsoredUsageState::Ambiguous
                    ) || command.observed_at < current.updated_at
                        || audit.occurred_at != command.observed_at
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if let Some(actual) = command.actual.as_ref() {
                        if command.actual_quota != Some(actual.quota)
                            || !actual.quota.fits_within(current.reserved.quota)
                            || actual.spend_micros > current.reserved.spend_micros
                        {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                    }
                    let mut replacement = current.clone();
                    replacement.state = command.outcome;
                    replacement.dispatch_lease = None;
                    replacement.actual = command.actual.clone();
                    replacement.updated_at = command.observed_at.clone();
                    replacement.validate().map_err(|error| {
                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                    })?;

                    if matches!(
                        command.outcome,
                        SponsoredUsageState::Settled | SponsoredUsageState::Released
                    ) {
                        let (mut grant, grant_document) = store
                            .grant_in_transaction(session, &current.grant_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                        let expected_generation = grant.generation;
                        let unresolved_usage =
                            summary_sub(&grant_document.unresolved_usage, &current.reserved)
                                .map_err(custom)?;
                        let capacity_was_released = grant_capacity_is_released(grant.state);

                        if let Some(actual) = command
                            .actual
                            .as_ref()
                            .filter(|_| command.outcome == SponsoredUsageState::Settled)
                        {
                            grant.usage = summary_add(&grant.usage, actual).map_err(custom)?;
                            if grant.updated_at < replacement.updated_at {
                                grant.updated_at = replacement.updated_at.clone();
                            }
                            let becomes_exhausted = !capacity_was_released
                                && matches!(
                                    grant.state,
                                    SponsoredGrantState::Active | SponsoredGrantState::Suspended
                                )
                                && grant.usage.reaches_any_limit(&grant.allocation);
                            let (policy, mut usage) = store
                                .policy_in_transaction(session, &grant.policy_id)
                                .await?
                                .ok_or_else(|| {
                                    custom(SponsorshipStoreError::Malformed(
                                        "stored grant references a missing sponsorship policy"
                                            .into(),
                                    ))
                                })?;
                            require_grant_policy_bindings(&grant, &policy).map_err(custom)?;
                            usage.consumed =
                                summary_add(&usage.consumed, actual).map_err(custom)?;
                            let released_hold = if capacity_was_released {
                                &current.reserved
                            } else {
                                actual
                            };
                            usage.reserved =
                                summary_sub(&usage.reserved, released_hold).map_err(custom)?;
                            store
                                .replace_policy_usage_in_transaction(session, &policy, &usage)
                                .await?;
                            if becomes_exhausted {
                                grant.generation =
                                    grant.generation.checked_add(1).ok_or_else(|| {
                                        custom(SponsorshipStoreError::Malformed(
                                            "grant generation exhausted".into(),
                                        ))
                                    })?;
                                grant.state = SponsoredGrantState::Exhausted;
                                grant.refresh_capability().map_err(|error| {
                                    custom(SponsorshipStoreError::Malformed(error.to_string()))
                                })?;
                                store
                                    .release_terminal_grant_capacity_in_transaction(
                                        session,
                                        &grant,
                                        &unresolved_usage,
                                    )
                                    .await?;
                            }
                        } else if capacity_was_released {
                            let (policy, mut usage) = store
                                .policy_in_transaction(session, &grant.policy_id)
                                .await?
                                .ok_or_else(|| {
                                    custom(SponsorshipStoreError::Malformed(
                                        "stored grant references a missing sponsorship policy"
                                            .into(),
                                    ))
                                })?;
                            require_grant_policy_bindings(&grant, &policy).map_err(custom)?;
                            usage.reserved =
                                summary_sub(&usage.reserved, &current.reserved).map_err(custom)?;
                            store
                                .replace_policy_usage_in_transaction(session, &policy, &usage)
                                .await?;
                        }
                        grant.validate().map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })?;
                        store
                            .replace_grant_in_transaction(
                                session,
                                expected_generation,
                                &grant,
                                &grant_document.sponsor_subject,
                                unresolved_usage,
                                grant_document.provider_receipt,
                                grant_document.encryption_key_id,
                            )
                            .await?;
                    }

                    let replacement_document = replace_reservation_for_reconciliation(
                        &store.cipher,
                        document,
                        &replacement,
                        command.model_response.as_ref(),
                    )
                    .map_err(custom)?;
                    let mut reservation_filter = doc! {
                        "_id": &current.id.0,
                        "state": usage_state_code(current.state),
                        "updated_at": &current.updated_at,
                        "reservation.dispatch_revision": signed(
                            current.dispatch_revision,
                            "usage dispatch revision",
                        )
                        .map_err(custom)?,
                    };
                    match current.dispatch_lease.as_ref() {
                        Some(lease) => {
                            reservation_filter
                                .insert("reservation.dispatch_lease.holder", lease.holder.clone());
                            reservation_filter.insert(
                                "reservation.dispatch_lease.expires_at",
                                lease.expires_at.clone(),
                            );
                        }
                        None => {
                            reservation_filter
                                .insert("reservation.dispatch_lease", doc! { "$exists": false });
                        }
                    }
                    let result = store
                        .reservations
                        .replace_one(reservation_filter, replacement_document)
                        .session(&mut *session)
                        .await?;
                    if result.matched_count != 1 {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    store.audit_in_transaction(session, audit).await?;
                    Ok(replacement)
                }
                .boxed()
            },
        )
        .await
    }

    async fn credential_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredCredentialId>, SponsorshipStoreError> {
        const DOMAIN: &str = "credential_recovery";
        let filter = recovery_page_filter(
            doc! { "state": { "$in": ["rotation_pending", "revocation_pending"] } },
            page,
            DOMAIN,
        )?;
        let documents = self
            .credentials
            .find(filter)
            .sort(doc! { "updated_at": 1, "_id": 1 })
            .limit(recovery_query_limit(page))
            .await
            .map_err(|error| unavailable("discover credential recovery work", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read credential recovery work", error))?;
        let mut rows = Vec::with_capacity(documents.len());
        for document in documents {
            let record = read_credential_record(&self.cipher, &self.operational, &document)?;
            if !matches!(
                record.state,
                SponsoredCredentialState::RotationPending
                    | SponsoredCredentialState::RevocationPending
            ) {
                return Err(SponsorshipStoreError::Malformed(
                    "credential recovery index contains a terminal row".into(),
                ));
            }
            rows.push((record.updated_at, record.id));
        }
        let page = finish_recovery_page(rows, page, DOMAIN, |row| (&row.0, &row.1 .0))?;
        Ok(SponsorshipRecoveryPage {
            items: page.items.into_iter().map(|(_, id)| id).collect(),
            next: page.next,
        })
    }

    async fn activation_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<LaunchId>, SponsorshipStoreError> {
        const DOMAIN: &str = "activation_recovery";
        let states = [
            "billing_attempting",
            "billing_ambiguous",
            "provider_attempting",
            "provider_ambiguous",
            "provider_confirmed",
            "cleanup_pending",
        ];
        let filter = recovery_page_filter(doc! { "state": { "$in": states } }, page, DOMAIN)?;
        let documents = self
            .activations
            .find(filter)
            .sort(doc! { "updated_at": 1, "_id": 1 })
            .limit(recovery_query_limit(page))
            .await
            .map_err(|error| unavailable("discover activation recovery work", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read activation recovery work", error))?;
        let mut rows = Vec::with_capacity(documents.len());
        for document in documents {
            let (intent, _) = read_activation(&self.cipher, &self.operational, document)?;
            if !matches!(
                intent.state,
                GrantActivationState::BillingAttempting
                    | GrantActivationState::BillingAmbiguous
                    | GrantActivationState::ProviderAttempting
                    | GrantActivationState::ProviderAmbiguous
                    | GrantActivationState::ProviderConfirmed
                    | GrantActivationState::CleanupPending
            ) {
                return Err(SponsorshipStoreError::Malformed(
                    "activation recovery index contains a terminal row".into(),
                ));
            }
            rows.push((intent.updated_at, intent.launch_id));
        }
        let page = finish_recovery_page(rows, page, DOMAIN, |row| (&row.0, &row.1 .0))?;
        Ok(SponsorshipRecoveryPage {
            items: page.items.into_iter().map(|(_, id)| id).collect(),
            next: page.next,
        })
    }

    async fn grant_recovery_candidates(
        &self,
        as_of: &str,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredGrantId>, SponsorshipStoreError> {
        if layer2_core::seconds_from_rfc3339(as_of).is_none() {
            return Err(SponsorshipStoreError::Malformed(
                "grant recovery clock fence is not a CMA timestamp".into(),
            ));
        }
        let domain = format!("grant_recovery:{as_of}");
        let revocation_filter = recovery_page_filter_on(
            doc! {
                "state": "revocation_pending",
                "updated_at": { "$lte": as_of },
            },
            page,
            &domain,
            "updated_at",
        )?;
        let revocation_documents = self
            .grants
            .find(revocation_filter)
            .sort(doc! { "updated_at": 1, "_id": 1 })
            .hint(Hint::Name("sponsorship_grants_recovery".into()))
            .limit(recovery_query_limit(page))
            .await
            .map_err(|error| unavailable("discover grant revocation recovery work", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read grant revocation recovery work", error))?;
        let mut rows =
            Vec::with_capacity(revocation_documents.len() + 2 * (page.limit() as usize + 1));
        for document in revocation_documents {
            let grant = read_grant_record(&self.operational, &document)?;
            if grant.state != SponsoredGrantState::RevocationPending
                || grant.updated_at.as_str() > as_of
            {
                return Err(SponsorshipStoreError::Malformed(
                    "grant revocation recovery index contains an ineligible row".into(),
                ));
            }
            rows.push((grant.updated_at, grant.id));
        }

        for (state_code, expected_state) in [
            ("active", SponsoredGrantState::Active),
            ("suspended", SponsoredGrantState::Suspended),
        ] {
            let expiry_filter = recovery_page_filter_on(
                doc! {
                    "state": state_code,
                    "expires_at": { "$lte": as_of },
                },
                page,
                &domain,
                "expires_at",
            )?;
            let documents = self
                .grants
                .find(expiry_filter)
                .sort(doc! { "expires_at": 1, "_id": 1 })
                .hint(Hint::Name("sponsorship_grants_due".into()))
                .limit(recovery_query_limit(page))
                .await
                .map_err(|error| unavailable("discover due grant expiry work", error))?
                .try_collect::<Vec<_>>()
                .await
                .map_err(|error| unavailable("read due grant expiry work", error))?;
            for document in documents {
                let grant = read_grant_record(&self.operational, &document)?;
                if grant.state != expected_state || grant.authority.expires_at.as_str() > as_of {
                    return Err(SponsorshipStoreError::Malformed(
                        "grant expiry index contains an ineligible row".into(),
                    ));
                }
                rows.push((grant.authority.expires_at, grant.id));
            }
        }

        rows.sort_by(|left, right| (&left.0, &left.1 .0).cmp(&(&right.0, &right.1 .0)));
        let page = finish_recovery_page(rows, page, &domain, |row| (&row.0, &row.1 .0))?;
        Ok(SponsorshipRecoveryPage {
            items: page.items.into_iter().map(|(_, id)| id).collect(),
            next: page.next,
        })
    }

    async fn usage_recovery_candidates(
        &self,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsoredUsageReservationId>, SponsorshipStoreError> {
        const DOMAIN: &str = "usage_recovery";
        let filter = recovery_page_filter(
            doc! {
                "state": { "$in": ["reserved", "attempting", "ambiguous"] },
                "model_dispatch_payload": { "$exists": true },
            },
            page,
            DOMAIN,
        )?;
        let documents = self
            .reservations
            .clone_with_type::<ReservationRecoveryKeyDoc>()
            .find(filter)
            .sort(doc! { "updated_at": 1, "_id": 1 })
            .projection(doc! { "_id": 1, "updated_at": 1 })
            .limit(recovery_query_limit(page))
            .await
            .map_err(|error| unavailable("discover usage recovery work", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| unavailable("read usage recovery work", error))?;
        let mut rows = Vec::with_capacity(documents.len());
        for document in documents {
            timestamp_date_time(&document.updated_at, "usage recovery update")?;
            let id = SponsoredUsageReservationId::parse(document.reservation_id)
                .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
            rows.push((document.updated_at, id));
        }
        let page = finish_recovery_page(rows, page, DOMAIN, |row| (&row.0, &row.1 .0))?;
        Ok(SponsorshipRecoveryPage {
            items: page.items.into_iter().map(|(_, id)| id).collect(),
            next: page.next,
        })
    }

    async fn model_usage_recovery_record(
        &self,
        id: &SponsoredUsageReservationId,
    ) -> Result<Option<SponsoredModelRecoveryRecord>, SponsorshipStoreError> {
        SponsoredUsageReservationId::parse(id.0.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let Some(document) = self
            .reservations
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read model usage recovery record", error))?
        else {
            return Ok(None);
        };
        let (reservation, policy_id, _) = read_reservation(&self.operational, document.clone())?;
        if !matches!(
            reservation.state,
            SponsoredUsageState::Reserved
                | SponsoredUsageState::Attempting
                | SponsoredUsageState::Ambiguous
        ) {
            return Ok(None);
        }
        let grant = self.grant(&reservation.grant_id).await?.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "model usage recovery references a missing grant".into(),
            )
        })?;
        let policy = self.policy(&policy_id).await?.ok_or_else(|| {
            SponsorshipStoreError::Malformed(
                "model usage recovery references a missing policy".into(),
            )
        })?;
        require_grant_policy_bindings(&grant, &policy)?;
        if grant.policy_id != policy_id
            || document.credential_id.as_deref() != Some(policy.credential_id.0.as_str())
        {
            return Err(SponsorshipStoreError::Malformed(
                "model usage recovery authority indexes are inconsistent".into(),
            ));
        }
        let Some(dispatch) = read_model_dispatch(&self.cipher, &document)? else {
            return Ok(None);
        };
        Ok(Some(SponsoredModelRecoveryRecord {
            reservation,
            dispatch,
        }))
    }

    async fn model_usage_settled_response(
        &self,
        id: &SponsoredUsageReservationId,
    ) -> Result<Option<SponsoredModelResponsePayload>, SponsorshipStoreError> {
        SponsoredUsageReservationId::parse(id.0.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        let Some(document) = self
            .reservations
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|error| stored_read_failure("read settled model response", error))?
        else {
            return Ok(None);
        };
        let (reservation, _, _) = read_reservation(&self.operational, document.clone())?;
        if reservation.state != SponsoredUsageState::Settled {
            return Ok(None);
        }
        read_model_response(&self.cipher, &document)
    }

    async fn ciphertext_rewrap_candidates(
        &self,
        kind: SponsorshipCiphertextRecordKind,
        retiring_key_id: &str,
        page: &SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<SponsorshipCiphertextRewrapCandidate>, SponsorshipStoreError>
    {
        validate_key_id(retiring_key_id).map_err(SponsorshipStoreError::Malformed)?;
        if retiring_key_id == self.cipher.current_key_id() {
            return Ok(SponsorshipRecoveryPage {
                items: Vec::new(),
                next: None,
            });
        }
        let bounded_page = (kind == SponsorshipCiphertextRecordKind::UsageReservation).then(|| {
            SponsorshipRecoveryPageRequest::new(
                page.limit().min(MAX_USAGE_REWRAP_PAGE),
                page.cursor().map(str::to_owned),
            )
        });
        let page = bounded_page.as_ref().unwrap_or(page);
        let domain = rewrap_domain(kind, retiring_key_id);
        let base = doc! { "encryption_key_id": retiring_key_id };
        let filter = rewrap_page_filter(base, page, &domain)?;
        let mut items = Vec::new();
        match kind {
            SponsorshipCiphertextRecordKind::Credential => {
                let documents = self
                    .credentials
                    .clone_with_type::<Document>()
                    .find(filter)
                    .sort(doc! { "_id": 1 })
                    .limit(recovery_query_limit(page))
                    .await
                    .map_err(|error| unavailable("discover credential rewrap work", error))?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(|error| unavailable("read credential rewrap work", error))?;
                for raw_document in documents {
                    let document_fingerprint = rewrap_document_fingerprint(kind, &raw_document)?;
                    let document: CredentialDoc =
                        decode_rewrap_document(raw_document, "sponsorship credential")?;
                    SponsoredCredentialId::parse(document.id.clone())
                        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                    if document.encryption_key_id != retiring_key_id
                        || document.provider_policy_handle.key_id() != retiring_key_id
                        || document
                            .current_secret
                            .as_ref()
                            .is_some_and(|envelope| envelope.key_id() != retiring_key_id)
                        || document
                            .pending_secret
                            .as_ref()
                            .is_some_and(|envelope| envelope.key_id() != retiring_key_id)
                    {
                        return Err(SponsorshipStoreError::Malformed(
                            "credential rewrap index does not match its envelopes".into(),
                        ));
                    }
                    items.push(SponsorshipCiphertextRewrapCandidate {
                        kind,
                        record_id: document.id,
                        retiring_key_id: retiring_key_id.to_string(),
                        document_fingerprint,
                    });
                }
            }
            SponsorshipCiphertextRecordKind::GrantActivation => {
                let documents = self
                    .activations
                    .clone_with_type::<Document>()
                    .find(filter)
                    .sort(doc! { "_id": 1 })
                    .limit(recovery_query_limit(page))
                    .await
                    .map_err(|error| unavailable("discover activation rewrap work", error))?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(|error| unavailable("read activation rewrap work", error))?;
                for raw_document in documents {
                    let document_fingerprint = rewrap_document_fingerprint(kind, &raw_document)?;
                    let document: ActivationDoc =
                        decode_rewrap_document(raw_document, "sponsorship grant activation")?;
                    LaunchId::parse(document.launch_id.clone())
                        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                    if document.encryption_key_id != retiring_key_id
                        || document.provider_receipt.as_ref().is_some_and(|receipt| {
                            receipt.provider_grant_id.key_id() != retiring_key_id
                                || receipt.revocation_handle.key_id() != retiring_key_id
                        })
                    {
                        return Err(SponsorshipStoreError::Malformed(
                            "activation rewrap index does not match its envelopes".into(),
                        ));
                    }
                    items.push(SponsorshipCiphertextRewrapCandidate {
                        kind,
                        record_id: document.launch_id,
                        retiring_key_id: retiring_key_id.to_string(),
                        document_fingerprint,
                    });
                }
            }
            SponsorshipCiphertextRecordKind::Grant => {
                let documents = self
                    .grants
                    .clone_with_type::<Document>()
                    .find(filter)
                    .sort(doc! { "_id": 1 })
                    .limit(recovery_query_limit(page))
                    .await
                    .map_err(|error| unavailable("discover grant rewrap work", error))?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(|error| unavailable("read grant rewrap work", error))?;
                for raw_document in documents {
                    let document_fingerprint = rewrap_document_fingerprint(kind, &raw_document)?;
                    let document: GrantDoc =
                        decode_rewrap_document(raw_document, "sponsored grant")?;
                    SponsoredGrantId::parse(document.id.clone())
                        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                    if document.encryption_key_id != retiring_key_id
                        || document.provider_receipt.provider_grant_id.key_id() != retiring_key_id
                        || document.provider_receipt.revocation_handle.key_id() != retiring_key_id
                    {
                        return Err(SponsorshipStoreError::Malformed(
                            "grant rewrap index does not match its envelopes".into(),
                        ));
                    }
                    items.push(SponsorshipCiphertextRewrapCandidate {
                        kind,
                        record_id: document.id,
                        retiring_key_id: retiring_key_id.to_string(),
                        document_fingerprint,
                    });
                }
            }
            SponsorshipCiphertextRecordKind::UsageReservation => {
                let documents = self
                    .reservations
                    .clone_with_type::<Document>()
                    .find(filter)
                    .sort(doc! { "_id": 1 })
                    .limit(recovery_query_limit(page))
                    .await
                    .map_err(|error| unavailable("discover usage rewrap work", error))?
                    .try_collect::<Vec<_>>()
                    .await
                    .map_err(|error| unavailable("read usage rewrap work", error))?;
                for raw_document in documents {
                    let document_fingerprint = rewrap_document_fingerprint(kind, &raw_document)?;
                    let document: ReservationDoc =
                        decode_rewrap_document(raw_document, "sponsored usage reservation")?;
                    SponsoredUsageReservationId::parse(document.reservation_id.clone())
                        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                    validate_reservation_custody(&document)?;
                    let envelope = document
                        .model_dispatch_payload
                        .as_ref()
                        .or(document.model_response_payload.as_ref());
                    if document.encryption_key_id.as_deref() != Some(retiring_key_id)
                        || envelope.is_none_or(|envelope| envelope.key_id() != retiring_key_id)
                    {
                        return Err(SponsorshipStoreError::Malformed(
                            "usage rewrap index does not match its envelope".into(),
                        ));
                    }
                    items.push(SponsorshipCiphertextRewrapCandidate {
                        kind,
                        record_id: document.reservation_id,
                        retiring_key_id: retiring_key_id.to_string(),
                        document_fingerprint,
                    });
                }
            }
        }
        finish_rewrap_page(items, page, &domain)
    }

    async fn rewrap_ciphertext(
        &self,
        candidate: &SponsorshipCiphertextRewrapCandidate,
    ) -> Result<bool, SponsorshipStoreError> {
        validate_key_id(&candidate.retiring_key_id).map_err(SponsorshipStoreError::Malformed)?;
        if !is_digest(&candidate.document_fingerprint) {
            return Err(SponsorshipStoreError::Malformed(
                "sponsorship rewrap document fingerprint is not a digest".into(),
            ));
        }
        if candidate.retiring_key_id == self.cipher.current_key_id() {
            return Ok(false);
        }
        let lookup_filter = doc! {
            "_id": &candidate.record_id,
            "encryption_key_id": &candidate.retiring_key_id,
        };
        let current_key_id = self.cipher.current_key_id().to_string();
        let modified = match candidate.kind {
            SponsorshipCiphertextRecordKind::Credential => {
                SponsoredCredentialId::parse(candidate.record_id.clone())
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                let Some(raw_document) = self
                    .credentials
                    .clone_with_type::<Document>()
                    .find_one(lookup_filter.clone())
                    .await
                    .map_err(|error| stored_read_failure("read credential for rewrap", error))?
                else {
                    return Ok(false);
                };
                if rewrap_document_fingerprint(candidate.kind, &raw_document)?
                    != candidate.document_fingerprint
                {
                    return Ok(false);
                }
                let filter = rewrap_exact_document_filter(
                    &raw_document,
                    &candidate.record_id,
                    &candidate.retiring_key_id,
                );
                let document: CredentialDoc =
                    decode_rewrap_document(raw_document, "sponsorship credential")?;
                let record = read_credential_record(&self.cipher, &self.operational, &document)?;
                let context = credential_cipher_context(
                    &document.id,
                    &record.owner,
                    &record.authority.provider,
                    &record.authority.provider_policy_key,
                    &record.authority.provider_credential_id_digest,
                    &record.authority.ownership_verified_at,
                    &record.created_at,
                    record.authority.state_version,
                    record.authority.supports_expiry,
                    record.authority.supports_revocation,
                )?;
                let required = |field: &str,
                                envelope: &SponsorshipCipherEnvelope|
                 -> Result<SponsorshipCipherEnvelope, SponsorshipStoreError> {
                    self.cipher
                        .rewrap("credential", &document.id, field, &context, envelope)?
                        .ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "credential rewrap key index is already current".into(),
                            )
                        })
                };
                let mut set = Document::new();
                set.insert(
                    "provider_policy_handle",
                    envelope_bson(&required(
                        "provider_policy_handle",
                        &document.provider_policy_handle,
                    )?)?,
                );
                if let Some(envelope) = &document.current_secret {
                    set.insert(
                        "current_secret",
                        envelope_bson(&required("current_secret", envelope)?)?,
                    );
                }
                if let Some(envelope) = &document.pending_secret {
                    set.insert(
                        "pending_secret",
                        envelope_bson(&required("pending_secret", envelope)?)?,
                    );
                }
                set.insert("encryption_key_id", current_key_id.clone());
                self.credentials
                    .update_one(filter, doc! { "$set": set })
                    .await
                    .map_err(|error| unavailable("rewrap credential ciphertext", error))?
                    .modified_count
            }
            SponsorshipCiphertextRecordKind::GrantActivation => {
                LaunchId::parse(candidate.record_id.clone())
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                let Some(raw_document) = self
                    .activations
                    .clone_with_type::<Document>()
                    .find_one(lookup_filter.clone())
                    .await
                    .map_err(|error| stored_read_failure("read activation for rewrap", error))?
                else {
                    return Ok(false);
                };
                if rewrap_document_fingerprint(candidate.kind, &raw_document)?
                    != candidate.document_fingerprint
                {
                    return Ok(false);
                }
                let filter = rewrap_exact_document_filter(
                    &raw_document,
                    &candidate.record_id,
                    &candidate.retiring_key_id,
                );
                let document: ActivationDoc =
                    decode_rewrap_document(raw_document, "sponsorship grant activation")?;
                let (intent, _) =
                    read_activation(&self.cipher, &self.operational, document.clone())?;
                let context = activation_cipher_context(
                    &intent.launch_id.0,
                    &intent.session_id.0,
                    &intent.policy_id.0,
                    intent.policy_generation,
                    intent.credential_generation,
                    &intent.launch_authority_digest,
                    &intent.acceptance_digest,
                    &intent.gate_fence,
                    &intent.allocation,
                    &intent.request_fingerprint,
                    &intent.billing_request_id,
                    &intent.provider_request_id,
                    &intent.created_at,
                )?;
                let mut set = Document::new();
                if let Some(mut receipt) = document.provider_receipt {
                    receipt.provider_grant_id = self
                        .cipher
                        .rewrap(
                            "activation",
                            &document.launch_id,
                            "provider_grant_id",
                            &context,
                            &receipt.provider_grant_id,
                        )?
                        .ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "activation rewrap key index is already current".into(),
                            )
                        })?;
                    receipt.revocation_handle = self
                        .cipher
                        .rewrap(
                            "activation",
                            &document.launch_id,
                            "revocation_handle",
                            &context,
                            &receipt.revocation_handle,
                        )?
                        .ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "activation rewrap key index is already current".into(),
                            )
                        })?;
                    set.insert(
                        "provider_receipt",
                        mongodb::bson::serialize_to_bson(&receipt).map_err(|_| {
                            SponsorshipStoreError::Malformed(
                                "rewrapped activation receipt cannot be serialized".into(),
                            )
                        })?,
                    );
                }
                set.insert("encryption_key_id", current_key_id.clone());
                self.activations
                    .update_one(filter, doc! { "$set": set })
                    .await
                    .map_err(|error| unavailable("rewrap activation ciphertext", error))?
                    .modified_count
            }
            SponsorshipCiphertextRecordKind::Grant => {
                SponsoredGrantId::parse(candidate.record_id.clone())
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                let Some(raw_document) = self
                    .grants
                    .clone_with_type::<Document>()
                    .find_one(lookup_filter)
                    .await
                    .map_err(|error| stored_read_failure("read grant for rewrap", error))?
                else {
                    return Ok(false);
                };
                if rewrap_document_fingerprint(candidate.kind, &raw_document)?
                    != candidate.document_fingerprint
                {
                    return Ok(false);
                }
                let filter = rewrap_exact_document_filter(
                    &raw_document,
                    &candidate.record_id,
                    &candidate.retiring_key_id,
                );
                let document: GrantDoc = decode_rewrap_document(raw_document, "sponsored grant")?;
                let (grant, _) = read_grant(&self.cipher, &self.operational, document.clone())?;
                let context = grant_cipher_context(&grant)?;
                let mut receipt = document.provider_receipt;
                receipt.provider_grant_id = self
                    .cipher
                    .rewrap(
                        "grant",
                        &document.id,
                        "provider_grant_id",
                        &context,
                        &receipt.provider_grant_id,
                    )?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "grant rewrap key index is already current".into(),
                        )
                    })?;
                receipt.revocation_handle = self
                    .cipher
                    .rewrap(
                        "grant",
                        &document.id,
                        "revocation_handle",
                        &context,
                        &receipt.revocation_handle,
                    )?
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "grant rewrap key index is already current".into(),
                        )
                    })?;
                self.grants
                    .update_one(
                        filter,
                        doc! {
                            "$set": {
                                "provider_receipt": mongodb::bson::serialize_to_bson(&receipt)
                                    .map_err(|_| SponsorshipStoreError::Malformed(
                                        "rewrapped grant receipt cannot be serialized".into()
                                    ))?,
                                "encryption_key_id": current_key_id,
                            },
                        },
                    )
                    .await
                    .map_err(|error| unavailable("rewrap grant ciphertext", error))?
                    .modified_count
            }
            SponsorshipCiphertextRecordKind::UsageReservation => {
                SponsoredUsageReservationId::parse(candidate.record_id.clone())
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                let Some(raw_document) = self
                    .reservations
                    .clone_with_type::<Document>()
                    .find_one(lookup_filter)
                    .await
                    .map_err(|error| stored_read_failure("read usage for rewrap", error))?
                else {
                    return Ok(false);
                };
                if rewrap_document_fingerprint(candidate.kind, &raw_document)?
                    != candidate.document_fingerprint
                {
                    return Ok(false);
                }
                let filter = rewrap_exact_document_filter(
                    &raw_document,
                    &candidate.record_id,
                    &candidate.retiring_key_id,
                );
                let document: ReservationDoc =
                    decode_rewrap_document(raw_document, "sponsored usage reservation")?;
                let (reservation, _, _) = read_reservation(&self.operational, document.clone())?;
                let request_digest = document
                    .model_dispatch_request_digest
                    .as_deref()
                    .ok_or_else(|| {
                        SponsorshipStoreError::Malformed(
                            "usage rewrap request digest is missing".into(),
                        )
                    })?;
                let (field, rewrapped) = if let Some(envelope) =
                    document.model_dispatch_payload.as_ref()
                {
                    let schema_version =
                        u32::try_from(document.model_dispatch_schema_version.ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "usage request rewrap schema is missing".into(),
                            )
                        })?)
                        .map_err(|_| {
                            SponsorshipStoreError::Malformed(
                                "usage request rewrap schema is invalid".into(),
                            )
                        })?;
                    let credential_id = SponsoredCredentialId::parse(
                        document.credential_id.clone().ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "usage rewrap credential id is missing".into(),
                            )
                        })?,
                    )
                    .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
                    let context = model_dispatch_cipher_context(
                        &reservation,
                        &credential_id,
                        schema_version,
                        request_digest,
                    )?;
                    (
                        "model_dispatch_payload",
                        self.cipher.rewrap_model_request(
                            "usage_reservation",
                            &candidate.record_id,
                            "model_dispatch_payload",
                            &context,
                            envelope,
                        )?,
                    )
                } else if let Some(envelope) = document.model_response_payload.as_ref() {
                    let schema_version =
                        u32::try_from(document.model_response_schema_version.ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "usage response rewrap schema is missing".into(),
                            )
                        })?)
                        .map_err(|_| {
                            SponsorshipStoreError::Malformed(
                                "usage response rewrap schema is invalid".into(),
                            )
                        })?;
                    let response_digest =
                        document.model_response_digest.as_deref().ok_or_else(|| {
                            SponsorshipStoreError::Malformed(
                                "usage response rewrap digest is missing".into(),
                            )
                        })?;
                    let context = model_response_cipher_context(
                        &reservation,
                        schema_version,
                        request_digest,
                        response_digest,
                    )?;
                    (
                        "model_response_payload",
                        self.cipher.rewrap_model_response(
                            "usage_reservation",
                            &candidate.record_id,
                            "model_response_payload",
                            &context,
                            envelope,
                        )?,
                    )
                } else {
                    return Err(SponsorshipStoreError::Malformed(
                        "usage rewrap payload is missing".into(),
                    ));
                };
                let rewrapped = rewrapped.ok_or_else(|| {
                    SponsorshipStoreError::Malformed(
                        "usage rewrap key index is already current".into(),
                    )
                })?;
                let mut set = Document::new();
                set.insert(field, envelope_bson(&rewrapped)?);
                set.insert("encryption_key_id", current_key_id);
                self.reservations
                    .update_one(filter, doc! { "$set": set })
                    .await
                    .map_err(|error| unavailable("rewrap usage ciphertext", error))?
                    .modified_count
            }
        };
        Ok(modified == 1)
    }

    async fn audit_events(
        &self,
        actor_digest: &str,
        limit: usize,
    ) -> Result<Vec<SponsoredAuditEvent>, SponsorshipStoreError> {
        if !is_digest(actor_digest) {
            return Err(SponsorshipStoreError::Malformed(
                "sponsorship audit actor is not a digest".into(),
            ));
        }
        let limit = limit.min(MAX_SPONSORED_AUDIT_PAGE);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let documents = self
            .audits
            .find(doc! { "actor_digest": actor_digest })
            .sort(doc! { "occurred_at": -1, "_id": -1 })
            .limit(i64::try_from(limit).map_err(|_| {
                SponsorshipStoreError::Malformed("sponsorship audit page is too large".into())
            })?)
            .await
            .map_err(|error| stored_read_failure("read sponsorship audit events", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| stored_read_failure("read sponsorship audit events", error))?;
        documents.into_iter().map(read_audit).collect()
    }
}

async fn validate_transaction_topology(database: &Database) -> Result<(), SponsorshipStoreError> {
    let reply = match database.run_command(doc! { "hello": 1 }).await {
        Ok(reply) => reply,
        Err(hello_error) => database
            .run_command(doc! { "isMaster": 1 })
            .await
            .map_err(|legacy_error| {
                SponsorshipStoreError::Unavailable(format!(
                    "validate sponsorship Mongo topology: hello failed ({hello_error}); isMaster failed ({legacy_error})"
                ))
            })?,
    };
    validate_transaction_topology_reply(&reply)
}

fn validate_transaction_topology_reply(reply: &Document) -> Result<(), SponsorshipStoreError> {
    let replica_set = matches!(reply.get("setName"), Some(Bson::String(name)) if !name.is_empty());
    let mongos = matches!(reply.get("msg"), Some(Bson::String(message)) if message == "isdbgrid");
    if replica_set || mongos {
        return Ok(());
    }
    Err(SponsorshipStoreError::Unavailable(
        "Mongo sponsorship persistence requires a replica set or mongos; standalone deployments cannot enforce its transactions".into(),
    ))
}

fn startup_gate_is_compatible(
    configured: &SponsorshipGateFence,
    stored: &SponsorshipGateFence,
) -> bool {
    configured == stored
        || (configured.configuration_digest == stored.configuration_digest
            && configured.trusted_provider_policy_key == stored.trusted_provider_policy_key
            && configured.runtime_limits == stored.runtime_limits
            && !configured.kill_switch_engaged
            && stored.kill_switch_engaged
            && configured.epoch.checked_add(1) == Some(stored.epoch))
}

fn gate_doc(fence: &SponsorshipGateFence) -> Result<GateDoc, SponsorshipStoreError> {
    validate_gate(fence)?;
    let engagement_id = fence.kill_switch_engaged.then(|| {
        deterministic_uuid(
            "cma-sponsorship-initial-kill-switch-v1",
            &[&fence.configuration_digest, &fence.epoch.to_string()],
        )
    });
    Ok(GateDoc {
        id: GATE_ID.into(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        configuration_digest: fence.configuration_digest.clone(),
        epoch: signed(fence.epoch, "sponsorship gate epoch")?,
        kill_switch_engaged: fence.kill_switch_engaged,
        trusted_provider_policy_key: fence.trusted_provider_policy_key.clone(),
        runtime_limits: fence.runtime_limits.clone(),
        kill_switch_engagement_id: engagement_id,
        kill_switch_drain_complete: true,
        authority_read_revision: 0,
    })
}

fn read_gate(document: GateDoc) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship gate")?;
    if document.id != GATE_ID
        || document.authority_read_revision < 0
        || (document.kill_switch_engaged != document.kill_switch_engagement_id.is_some())
        || (!document.kill_switch_engaged && !document.kill_switch_drain_complete)
        || document
            .kill_switch_engagement_id
            .as_ref()
            .is_some_and(|id| {
                uuid::Uuid::parse_str(id)
                    .map(|parsed| parsed.to_string() != *id)
                    .unwrap_or(true)
            })
    {
        return Err(SponsorshipStoreError::Malformed(
            "stored sponsorship gate has invalid identity, engagement, drain, or read metadata"
                .into(),
        ));
    }
    let fence = SponsorshipGateFence {
        configuration_digest: document.configuration_digest,
        epoch: unsigned(document.epoch, "sponsorship gate epoch")?,
        kill_switch_engaged: document.kill_switch_engaged,
        trusted_provider_policy_key: document.trusted_provider_policy_key,
        runtime_limits: document.runtime_limits,
    };
    validate_gate(&fence)?;
    Ok(fence)
}

fn deterministic_uuid(domain: &str, values: &[&str]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    for value in values {
        hasher.update([0]);
        hasher.update((value.len() as u64).to_be_bytes());
        hasher.update(value.as_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes).to_string()
}

fn validate_gate(fence: &SponsorshipGateFence) -> Result<(), SponsorshipStoreError> {
    fence
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))
}

fn validate_document_version(actual: i32, kind: &str) -> Result<(), SponsorshipStoreError> {
    if actual == DOCUMENT_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(SponsorshipStoreError::Malformed(format!(
            "stored {kind} document schema {actual} is unsupported"
        )))
    }
}

fn signed(value: u64, field: &str) -> Result<i64, SponsorshipStoreError> {
    i64::try_from(value).map_err(|_| {
        SponsorshipStoreError::Malformed(format!("{field} exceeds MongoDB's signed integer range"))
    })
}

fn unsigned(value: i64, field: &str) -> Result<u64, SponsorshipStoreError> {
    u64::try_from(value)
        .map_err(|_| SponsorshipStoreError::Malformed(format!("stored {field} is negative")))
}

fn named(name: &str) -> IndexOptions {
    IndexOptions::builder().name(name.to_string()).build()
}

fn unique_named(name: &str) -> IndexOptions {
    IndexOptions::builder()
        .name(name.to_string())
        .unique(true)
        .build()
}

fn unavailable(context: &str, error: Error) -> SponsorshipStoreError {
    SponsorshipStoreError::Unavailable(format!("{context}: {error}"))
}

fn stored_read_failure(context: &str, error: Error) -> SponsorshipStoreError {
    match error.kind.as_ref() {
        ErrorKind::BsonDeserialization(_) | ErrorKind::Bson(_) => SponsorshipStoreError::Malformed(
            format!("{context}: stored document cannot be decoded"),
        ),
        _ => unavailable(context, error),
    }
}

fn map_trigger_read_error(error: TriggerStoreError) -> SponsorshipStoreError {
    match error {
        TriggerStoreError::Unavailable(message) => SponsorshipStoreError::Unavailable(message),
        TriggerStoreError::Malformed(message) => SponsorshipStoreError::Malformed(message),
        other => SponsorshipStoreError::Malformed(format!(
            "the durable Launch cannot be decoded as sponsorship authority: {other}"
        )),
    }
}

fn map_transaction_result<T>(
    result: mongodb::error::Result<T>,
) -> Result<T, SponsorshipStoreError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            if let Some(custom) = error.get_custom::<SponsorshipStoreError>() {
                return Err(custom.clone());
            }
            if is_duplicate_key(&error) {
                return Err(SponsorshipStoreError::Conflict);
            }
            Err(stored_read_failure(
                "execute sponsorship persistence transaction",
                error,
            ))
        }
    }
}

fn is_duplicate_key(error: &Error) -> bool {
    match error.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(write)) => write.code == DUPLICATE_KEY,
        ErrorKind::Command(command) => command.code == DUPLICATE_KEY,
        _ => false,
    }
}

fn custom(error: SponsorshipStoreError) -> Error {
    Error::custom(error)
}

#[cfg(test)]
mod tests {
    use super::super::sponsorship_encryption::{
        SponsorshipEncryptionKey, SponsorshipOperationalKey,
    };
    use super::*;

    const TEST_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const TEST_ROTATED_KEY: &str = "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=";
    const TEST_NOW: &str = "2026-01-02T03:04:05Z";

    fn test_runtime_limits() -> SponsorshipRuntimeLimits {
        SponsorshipRuntimeLimits {
            default_policy_expiry_seconds: 3_600,
            maximum_policy_expiry_seconds: 86_400,
            maximum_active_policies_per_sponsor: 8,
            launch_rate_window_seconds: 60,
            maximum_launches_per_sponsor_per_window: 20,
            maximum_per_launch_quota: layer2_core::SponsoredQuota {
                requests: 100,
                input_tokens: 100_000,
                output_tokens: 100_000,
            },
            maximum_policy_quota: layer2_core::SponsoredQuota {
                requests: 1_000,
                input_tokens: 1_000_000,
                output_tokens: 1_000_000,
            },
            maximum_requests_per_minute: 60,
            maximum_per_launch_budget_micros: 100_000,
            maximum_policy_budget_micros: 1_000_000,
            maximum_concurrent_launches: 10,
        }
    }

    fn test_gate(digest_byte: char, epoch: u64, kill_switch_engaged: bool) -> SponsorshipGateFence {
        SponsorshipGateFence {
            configuration_digest: digest_byte.to_string().repeat(64),
            epoch,
            kill_switch_engaged,
            trusted_provider_policy_key: "default".into(),
            runtime_limits: test_runtime_limits(),
        }
    }

    fn test_keyring() -> SponsorshipKeyring {
        SponsorshipKeyring::new(
            "test-current",
            SponsorshipEncryptionKey::from_base64(TEST_KEY).unwrap(),
            Vec::new(),
        )
        .unwrap()
    }

    fn test_cipher() -> SponsorshipCipher {
        SponsorshipCipher::new("test-deployment", test_keyring()).unwrap()
    }

    fn test_operational_keyring() -> SponsorshipOperationalKeyring {
        SponsorshipOperationalKeyring::new(
            "test-operational-current",
            SponsorshipOperationalKey::from_bytes([0x22; 32]),
            Vec::new(),
        )
        .unwrap()
    }

    fn test_operational() -> SponsorshipOperationalPseudonymizer {
        test_operational_keyring()
            .pseudonymizer("test-deployment")
            .unwrap()
    }

    fn test_provider_identity() -> SponsorshipOperationalPseudonymSet {
        test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ProviderCredentialAuthority,
                &["openai", "provider-credential-id"],
            )
            .unwrap()
    }

    fn retiring_test_cipher() -> SponsorshipCipher {
        let keyring = SponsorshipKeyring::new(
            "test-retiring",
            SponsorshipEncryptionKey::from_base64(TEST_KEY).unwrap(),
            Vec::new(),
        )
        .unwrap();
        SponsorshipCipher::new("test-deployment", keyring).unwrap()
    }

    fn rotating_test_keyring() -> SponsorshipKeyring {
        SponsorshipKeyring::new(
            "test-current",
            SponsorshipEncryptionKey::from_base64(TEST_ROTATED_KEY).unwrap(),
            vec![(
                "test-retiring".into(),
                SponsorshipEncryptionKey::from_base64(TEST_KEY).unwrap(),
            )],
        )
        .unwrap()
    }

    fn rotated_test_keyring() -> SponsorshipKeyring {
        SponsorshipKeyring::new(
            "test-current",
            SponsorshipEncryptionKey::from_base64(TEST_ROTATED_KEY).unwrap(),
            Vec::new(),
        )
        .unwrap()
    }

    fn rotating_test_operational_keyring() -> SponsorshipOperationalKeyring {
        SponsorshipOperationalKeyring::new(
            "test-operational-rotated",
            SponsorshipOperationalKey::from_bytes([0x23; 32]),
            vec![(
                "test-operational-current".into(),
                SponsorshipOperationalKey::from_bytes([0x22; 32]),
            )],
        )
        .unwrap()
    }

    fn credential_fixture() -> SponsoredCredentialRecord {
        SponsoredCredentialRecord {
            schema_version: layer2_core::SPONSORSHIP_SCHEMA_VERSION,
            id: SponsoredCredentialId::new(),
            owner: SponsorBillingOwner::Developer {
                subject: SubjectId("sponsor-owner".into()),
                display_name: "Sponsor Owner".into(),
            },
            authority: ProviderCredentialAuthority {
                provider: "openai".into(),
                provider_policy_key: "default".into(),
                provider_policy_handle: SensitiveProviderValue::new("provider-policy-handle")
                    .unwrap(),
                provider_credential_id_digest: test_provider_identity()
                    .current()
                    .as_str()
                    .to_string(),
                state_version: 1,
                ownership_verified_at: TEST_NOW.into(),
                supports_expiry: true,
                supports_revocation: true,
            },
            state: SponsoredCredentialState::Active,
            material_state: SponsoredCredentialMaterialState::Retained,
            generation: 1,
            created_at: TEST_NOW.into(),
            updated_at: TEST_NOW.into(),
            revoked_at: None,
            pending_operation: None,
            superseded_operation: None,
            last_completed_operation: None,
        }
    }

    fn pending_operation_fixture(
        kind: ProviderOperationKind,
        phase: ProviderOperationPhase,
        requested_at: &str,
    ) -> PendingProviderOperation {
        let client_request_id_digest = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::CredentialOperationRequest,
                &["fixture", operation_kind_code(kind)],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        PendingProviderOperation {
            kind,
            phase,
            provider_request_id: "b".repeat(64),
            client_request_id_digest,
            payload_digest: "d".repeat(64),
            requested_at: requested_at.into(),
            attempted_at: (phase != ProviderOperationPhase::Prepared)
                .then(|| "2026-01-02T03:04:07Z".into()),
        }
    }

    fn rotation_pending_fixture(phase: ProviderOperationPhase) -> SponsoredCredentialRecord {
        let mut credential = credential_fixture();
        credential.state = SponsoredCredentialState::RotationPending;
        credential.generation = 2;
        credential.pending_operation = Some(pending_operation_fixture(
            ProviderOperationKind::RotateCredential,
            phase,
            "2026-01-02T03:04:06Z",
        ));
        credential.updated_at = if phase == ProviderOperationPhase::Prepared {
            "2026-01-02T03:04:06Z".into()
        } else {
            "2026-01-02T03:04:07Z".into()
        };
        credential.validate().unwrap();
        credential
    }

    #[test]
    fn credential_rotation_staging_only_accepts_active_to_prepared() {
        let current = credential_fixture();
        let mut candidate = rotation_pending_fixture(ProviderOperationPhase::Prepared);
        candidate.id = current.id.clone();
        candidate.owner = current.owner.clone();
        candidate.authority = current.authority.clone();
        candidate.created_at = current.created_at.clone();

        assert!(credential_rotation_stage_is_legal(&current, &candidate));

        let mut attempting = candidate.clone();
        let operation = attempting.pending_operation.as_mut().unwrap();
        operation.phase = ProviderOperationPhase::Attempting;
        operation.attempted_at = Some("2026-01-02T03:04:07Z".into());
        attempting.updated_at = "2026-01-02T03:04:07Z".into();
        assert!(!credential_rotation_stage_is_legal(&current, &attempting));

        let mut wrong_state = candidate;
        wrong_state.state = SponsoredCredentialState::RevocationPending;
        assert!(!credential_rotation_stage_is_legal(&current, &wrong_state));
    }

    #[test]
    fn credential_pending_operation_phase_matrix_is_closed() {
        let prepared = rotation_pending_fixture(ProviderOperationPhase::Prepared);
        let mut attempting = prepared.clone();
        attempting.generation += 1;
        attempting.updated_at = "2026-01-02T03:04:07Z".into();
        let operation = attempting.pending_operation.as_mut().unwrap();
        operation.phase = ProviderOperationPhase::Attempting;
        operation.attempted_at = Some("2026-01-02T03:04:07Z".into());
        assert!(credential_phase_update_is_legal(&prepared, &attempting));

        for phase in [
            ProviderOperationPhase::Prepared,
            ProviderOperationPhase::Ambiguous,
            ProviderOperationPhase::Rejected,
        ] {
            let mut candidate = attempting.clone();
            candidate.generation += 1;
            candidate.updated_at = "2026-01-02T03:04:08Z".into();
            let operation = candidate.pending_operation.as_mut().unwrap();
            operation.phase = phase;
            if phase == ProviderOperationPhase::Prepared {
                operation.attempted_at = None;
            }
            assert!(credential_phase_update_is_legal(&attempting, &candidate));
        }

        let mut repeated_attempt = attempting.clone();
        repeated_attempt.generation += 1;
        repeated_attempt.updated_at = "2026-01-02T03:04:08Z".into();
        assert!(!credential_phase_update_is_legal(
            &attempting,
            &repeated_attempt
        ));

        let mut cleared = attempting.clone();
        cleared.generation += 1;
        cleared.updated_at = "2026-01-02T03:04:08Z".into();
        cleared.pending_operation = None;
        assert!(!credential_phase_update_is_legal(&attempting, &cleared));
    }

    #[test]
    fn credential_pending_operation_identity_and_request_time_are_immutable() {
        let current = rotation_pending_fixture(ProviderOperationPhase::Prepared);
        let mut candidate = current.clone();
        candidate.generation += 1;
        candidate.updated_at = "2026-01-02T03:04:07Z".into();
        let operation = candidate.pending_operation.as_mut().unwrap();
        operation.phase = ProviderOperationPhase::Attempting;
        operation.attempted_at = Some("2026-01-02T03:04:07Z".into());

        let mut changed_provider_request = candidate.clone();
        changed_provider_request
            .pending_operation
            .as_mut()
            .unwrap()
            .provider_request_id = "e".repeat(64);
        assert!(!credential_phase_update_is_legal(
            &current,
            &changed_provider_request
        ));

        let mut changed_client_request = candidate.clone();
        changed_client_request
            .pending_operation
            .as_mut()
            .unwrap()
            .client_request_id_digest = "e".repeat(64);
        assert!(!credential_phase_update_is_legal(
            &current,
            &changed_client_request
        ));

        let mut changed_payload = candidate.clone();
        changed_payload
            .pending_operation
            .as_mut()
            .unwrap()
            .payload_digest = "e".repeat(64);
        assert!(!credential_phase_update_is_legal(
            &current,
            &changed_payload
        ));

        let mut changed_requested_at = candidate;
        changed_requested_at
            .pending_operation
            .as_mut()
            .unwrap()
            .requested_at = TEST_NOW.into();
        assert!(!credential_phase_update_is_legal(
            &current,
            &changed_requested_at
        ));
    }

    #[test]
    fn credential_rotation_can_only_be_superseded_by_exact_revocation_lineage() {
        let current = rotation_pending_fixture(ProviderOperationPhase::Attempting);
        let mut candidate = current.clone();
        candidate.generation += 1;
        candidate.state = SponsoredCredentialState::RevocationPending;
        candidate.updated_at = "2026-01-02T03:04:08Z".into();
        candidate.superseded_operation = current.pending_operation.clone();
        candidate.pending_operation = Some(pending_operation_fixture(
            ProviderOperationKind::RevokeCredential,
            ProviderOperationPhase::Prepared,
            "2026-01-02T03:04:08Z",
        ));
        candidate
            .pending_operation
            .as_mut()
            .unwrap()
            .provider_request_id = "e".repeat(64);
        assert!(credential_update_is_legal(&current, &candidate));

        let mut cleared = candidate.clone();
        cleared.superseded_operation = None;
        assert!(!credential_update_is_legal(&current, &cleared));

        let mut substituted = candidate;
        substituted
            .superseded_operation
            .as_mut()
            .unwrap()
            .provider_request_id = "f".repeat(64);
        assert!(!credential_update_is_legal(&current, &substituted));
    }

    #[test]
    fn credential_rotation_completion_requires_advanced_bound_authority() {
        let current = rotation_pending_fixture(ProviderOperationPhase::Attempting);
        let pending = current.pending_operation.as_ref().unwrap();
        let mut candidate = current.clone();
        candidate.generation += 1;
        candidate.state = SponsoredCredentialState::Active;
        candidate.pending_operation = None;
        candidate.updated_at = "2026-01-02T03:04:08Z".into();
        candidate.authority.provider_credential_id_digest = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ProviderCredentialAuthority,
                &["openai", "rotated-provider-credential-id"],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        candidate.authority.state_version += 1;
        candidate.authority.ownership_verified_at = "2026-01-02T03:04:07Z".into();
        candidate.last_completed_operation = Some(CompletedProviderOperation {
            kind: pending.kind,
            provider_request_id: pending.provider_request_id.clone(),
            client_request_id_digest: pending.client_request_id_digest.clone(),
            payload_digest: pending.payload_digest.clone(),
            outcome: CompletedProviderOperationOutcome::Confirmed,
            completed_at: candidate.updated_at.clone(),
        });
        candidate.validate().unwrap();

        assert!(credential_completion_shape_is_legal(
            &current,
            &candidate,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Confirmed,
        ));
        credential_operation_completion(
            &current,
            &candidate,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Confirmed,
        )
        .unwrap();

        let mut stale_authority = candidate.clone();
        stale_authority.authority.state_version = current.authority.state_version;
        assert!(!credential_completion_shape_is_legal(
            &current,
            &stale_authority,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Confirmed,
        ));

        let mut substituted_identity = candidate.clone();
        substituted_identity.authority.provider_credential_id_digest =
            current.authority.provider_credential_id_digest.clone();
        assert!(!credential_completion_shape_is_legal(
            &current,
            &substituted_identity,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Confirmed,
        ));

        let mut early_evidence = candidate.clone();
        early_evidence.authority.ownership_verified_at = TEST_NOW.into();
        assert!(!credential_completion_shape_is_legal(
            &current,
            &early_evidence,
            ProviderOperationKind::RotateCredential,
            CompletedProviderOperationOutcome::Confirmed,
        ));

        let mut early_completion = candidate;
        early_completion
            .last_completed_operation
            .as_mut()
            .unwrap()
            .completed_at = "2026-01-02T03:04:06Z".into();
        early_completion.updated_at = "2026-01-02T03:04:06Z".into();
        assert!(matches!(
            credential_operation_completion(
                &current,
                &early_completion,
                ProviderOperationKind::RotateCredential,
                CompletedProviderOperationOutcome::Confirmed,
            ),
            Err(SponsorshipStoreError::Conflict)
        ));
    }

    fn policy_fixture() -> SponsorshipPolicy {
        SponsorshipPolicy {
            schema_version: layer2_core::SPONSORSHIP_SCHEMA_VERSION,
            id: SponsorshipPolicyId::new(),
            tenant_id: "tenant-a".into(),
            owner: credential_fixture().owner,
            credential_id: SponsoredCredentialId::new(),
            provider_policy: layer2_core::SponsoredProviderPolicy {
                provider: "openai".into(),
                provider_policy_key: "default".into(),
                allowed_models: std::collections::BTreeSet::from(["gpt-5".into()]),
                allowed_resources: std::collections::BTreeSet::from(["responses".into()]),
            },
            per_launch_quota: layer2_core::SponsoredQuota {
                requests: 1,
                input_tokens: 10,
                output_tokens: 10,
            },
            aggregate_quota: layer2_core::SponsoredQuota {
                requests: 10,
                input_tokens: 100,
                output_tokens: 100,
            },
            per_launch_budget_micros: 100,
            aggregate_budget_micros: 1_000,
            max_requests_per_minute: 10,
            max_concurrent_launches: 2,
            expires_at: "2027-01-02T03:04:05Z".into(),
            state: layer2_core::SponsorshipPolicyState::Active,
            generation: 1,
            created_at: TEST_NOW.into(),
            updated_at: TEST_NOW.into(),
            revoked_at: None,
        }
    }

    fn grant_fixture(
        state: SponsoredGrantState,
        expires_at: &str,
    ) -> (SponsoredGrant, ProviderGrantReceipt) {
        let id = SponsoredGrantId::new();
        let launch_id = LaunchId::new();
        let session_id = SessionId(format!("session-{}", Uuid::new_v4().simple()));
        let policy_id = SponsorshipPolicyId::new();
        let revision_id = TriggerRevisionId::new();
        let contributor = SubjectId("grant-recovery-contributor".into());
        let provider_grant_id = format!("provider-grant-{}", Uuid::new_v4().simple());
        let revocation_handle = format!("provider-revocation-{}", Uuid::new_v4().simple());
        let issued_at = "2026-01-02T03:04:06Z";
        let created_at = "2026-01-02T03:04:07Z";
        let updated_at = "2026-01-02T03:04:08Z";
        let acceptance_idempotency = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::AcceptanceRequest,
                &[&launch_id.0, &contributor.0, "grant-fixture-acceptance"],
            )
            .unwrap();
        let mut acceptance = SponsoredGrantAcceptance {
            launch_id: launch_id.clone(),
            contributor: contributor.clone(),
            revision_id: revision_id.clone(),
            revision_digest: "1".repeat(64),
            policy_id: policy_id.clone(),
            policy_generation: 1,
            terms_digest: "2".repeat(64),
            launch_snapshot_digest: "3".repeat(64),
            accepted_at: TEST_NOW.into(),
            idempotency_key_hash: acceptance_idempotency.current().as_str().to_string(),
            digest: String::new(),
        };
        acceptance.digest = acceptance.calculate_digest().unwrap();
        let receipt = ProviderGrantReceipt {
            provider_grant_id: SensitiveProviderValue::new(provider_grant_id.clone()).unwrap(),
            provider_state_version: 1,
            expires_at: expires_at.into(),
            revocation_handle: SensitiveProviderValue::new(revocation_handle.clone()).unwrap(),
            issued_at: issued_at.into(),
        };
        let operational = test_operational();
        let provider_grant_id_digest = operational
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ProviderGrantAuthority,
                &[&id.0, &provider_grant_id],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        let revocation_handle_digest = operational
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ProviderRevocationAuthority,
                &[&id.0, &revocation_handle],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        let mut grant = SponsoredGrant {
            schema_version: layer2_core::SPONSORSHIP_SCHEMA_VERSION,
            id: id.clone(),
            tenant_id: "tenant-a".into(),
            policy_id,
            policy_generation: 1,
            credential_generation: 1,
            trigger_revision_id: revision_id,
            trigger_revision_digest: "1".repeat(64),
            launch_id: launch_id.clone(),
            session_id: session_id.clone(),
            contributor,
            sponsor_subject: SubjectId("grant-recovery-sponsor".into()),
            terms_digest: "2".repeat(64),
            acceptance,
            authority: ProviderGrantAuthority {
                provider_grant_id_digest,
                provider_state_version: 1,
                expires_at: expires_at.into(),
                revocation_handle_digest,
                issued_at: issued_at.into(),
            },
            capability: InertGrantCapability {
                grant_id: id,
                grant_generation: 1,
                policy_generation: 1,
                launch_id,
                session_id,
                expires_at: expires_at.into(),
                fingerprint: String::new(),
            },
            state,
            allocation: SponsoredUsageSummary {
                quota: layer2_core::SponsoredQuota {
                    requests: 10,
                    input_tokens: 1_000,
                    output_tokens: 1_000,
                },
                spend_micros: 10_000,
            },
            usage: SponsoredUsageSummary::default(),
            generation: 1,
            created_at: created_at.into(),
            updated_at: updated_at.into(),
            revoked_at: None,
            pending_revocation: None,
        };
        grant.refresh_capability().unwrap();
        grant.validate().unwrap();
        (grant, receipt)
    }

    fn usage_reservation_fixture(
        grant_id: SponsoredGrantId,
        state: SponsoredUsageState,
    ) -> SponsoredUsageReservation {
        let maximum = SponsoredUsageSummary {
            quota: layer2_core::SponsoredQuota {
                requests: 1,
                input_tokens: 10,
                output_tokens: 10,
            },
            spend_micros: 100,
        };
        let mut price_quote = layer2_core::SponsoredPriceQuote {
            provider: "openai".into(),
            model: "gpt-5".into(),
            resource: "responses".into(),
            maximum: maximum.clone(),
            pricing_generation: 1,
            digest: String::new(),
        };
        price_quote.digest = price_quote.calculate_digest().unwrap();
        let reservation = SponsoredUsageReservation {
            id: SponsoredUsageReservationId::new(),
            idempotency_fingerprint: "8".repeat(64),
            request_fingerprint: "9".repeat(64),
            grant_id,
            price_quote,
            reserved: maximum,
            state,
            dispatch_revision: 0,
            dispatch_lease: None,
            created_at: TEST_NOW.into(),
            updated_at: TEST_NOW.into(),
            actual: None,
        };
        reservation.validate().unwrap();
        reservation
    }

    fn activation_fixture(
        policy_id: &SponsorshipPolicyId,
        gate_fence: &SponsorshipGateFence,
        state: GrantActivationState,
        created_at: &str,
        capacity_held: bool,
    ) -> GrantActivationIntent {
        let launch_id = LaunchId::new();
        let operational = test_operational();
        let billing_request_id = operational
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ActivationRequest,
                &[&launch_id.0, "billing-fixture"],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        let provider_request_id = operational
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ActivationRequest,
                &[&launch_id.0, "provider-fixture"],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        let mut intent = GrantActivationIntent {
            launch_id: launch_id.clone(),
            session_id: SessionId(format!("session-{}", Uuid::new_v4().simple())),
            policy_id: policy_id.clone(),
            policy_generation: 1,
            credential_generation: 1,
            launch_authority_digest: "5".repeat(64),
            acceptance_digest: "6".repeat(64),
            gate_fence: gate_fence.clone(),
            allocation: SponsoredUsageSummary {
                quota: layer2_core::SponsoredQuota {
                    requests: 1,
                    input_tokens: 10,
                    output_tokens: 10,
                },
                spend_micros: 100,
            },
            request_fingerprint: "7".repeat(64),
            billing_request_id,
            provider_request_id,
            state,
            denial: None,
            capacity_held,
            revision: 1,
            lease: None,
            provider_receipt: None,
            cleanup_operation: None,
            created_at: created_at.into(),
            updated_at: created_at.into(),
        };
        if matches!(
            state,
            GrantActivationState::BillingAttempting | GrantActivationState::ProviderAttempting
        ) {
            let acquired = DateTime::parse_rfc3339_str(created_at).unwrap();
            let holder = operational
                .pseudonymize(
                    layer2_core::SponsorshipOperationalDomain::ActivationRequest,
                    &[&launch_id.0, "holder-fixture"],
                )
                .unwrap()
                .current()
                .as_str()
                .to_string();
            let (lease, _) = activation_lease(holder, acquired).unwrap();
            intent.lease = Some(lease);
        }
        intent.validate().unwrap();
        intent
    }

    fn kill_switch_audit(occurred_at: &str) -> SponsoredAuditEvent {
        SponsoredAuditEvent {
            event_id: Uuid::new_v4().to_string(),
            action: SponsoredAuditAction::KillSwitchEngaged,
            actor_digest: "a".repeat(64),
            policy_id: None,
            grant_id: None,
            stable_request_id_digest: None,
            outcome_code: "engaged".into(),
            occurred_at: occurred_at.into(),
        }
    }

    fn rewrap_public_projection(
        kind: SponsorshipCiphertextRecordKind,
        mut document: Document,
    ) -> Document {
        document.remove("encryption_key_id");
        match kind {
            SponsorshipCiphertextRecordKind::Credential => {
                document.remove("provider_policy_handle");
                document.remove("current_secret");
                document.remove("pending_secret");
            }
            SponsorshipCiphertextRecordKind::GrantActivation
            | SponsorshipCiphertextRecordKind::Grant => {
                if let Ok(receipt) = document.get_document_mut("provider_receipt") {
                    receipt.remove("provider_grant_id");
                    receipt.remove("revocation_handle");
                }
            }
            SponsorshipCiphertextRecordKind::UsageReservation => {
                document.remove("model_dispatch_payload");
                document.remove("model_response_payload");
            }
        }
        document
    }

    fn bson_tree_has_string(value: &Bson, key: &str, expected: &str) -> bool {
        match value {
            Bson::Document(document) => document.iter().any(|(candidate, value)| {
                (candidate == key && value.as_str() == Some(expected))
                    || bson_tree_has_string(value, key, expected)
            }),
            Bson::Array(values) => values
                .iter()
                .any(|value| bson_tree_has_string(value, key, expected)),
            _ => false,
        }
    }

    #[test]
    fn policy_updates_require_monotonic_transition_and_exact_revocation_time() {
        let current = policy_fixture();
        let mut suspended = current.clone();
        suspended.generation += 1;
        suspended.state = layer2_core::SponsorshipPolicyState::Suspended;
        suspended.updated_at = "2026-01-02T03:04:06Z".into();
        assert!(policy_update_is_legal(&current, &suspended));

        let mut regressed = suspended.clone();
        regressed.updated_at = "2026-01-02T03:04:04Z".into();
        assert!(!policy_update_is_legal(&current, &regressed));

        let mut revoked = suspended.clone();
        revoked.state = layer2_core::SponsorshipPolicyState::Revoked;
        revoked.generation += 1;
        revoked.updated_at = "2026-01-02T03:04:07Z".into();
        revoked.revoked_at = Some(revoked.updated_at.clone());
        assert!(policy_update_is_legal(&suspended, &revoked));

        revoked.revoked_at = Some("2026-01-02T03:04:06Z".into());
        assert!(!policy_update_is_legal(&suspended, &revoked));
    }

    #[test]
    fn sponsorship_topology_accepts_replica_sets_and_mongos_only() {
        validate_transaction_topology_reply(&doc! { "setName": "rs0" })
            .expect("a replica set supports transactions");
        validate_transaction_topology_reply(&doc! { "msg": "isdbgrid" })
            .expect("mongos supports transactions");

        let error = validate_transaction_topology_reply(&doc! { "isWritablePrimary": true })
            .expect_err("standalone Mongo must fail closed");
        assert!(matches!(error, SponsorshipStoreError::Unavailable(_)));
        assert!(error.to_string().contains("replica set or mongos"));
    }

    #[test]
    fn sponsorship_gate_schema_and_numeric_bounds_fail_closed() {
        let fence = test_gate('a', 1, true);
        assert_eq!(read_gate(gate_doc(&fence).unwrap()).unwrap(), fence);

        let mut unsupported = gate_doc(&fence).unwrap();
        unsupported.document_schema_version += 1;
        assert!(matches!(
            read_gate(unsupported),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut negative = gate_doc(&fence).unwrap();
        negative.epoch = -1;
        assert!(matches!(
            read_gate(negative),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut missing_engagement = gate_doc(&fence).unwrap();
        missing_engagement.kill_switch_engagement_id = None;
        assert!(matches!(
            read_gate(missing_engagement),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut malformed_engagement = gate_doc(&fence).unwrap();
        malformed_engagement.kill_switch_engagement_id = Some("not-a-uuid".into());
        assert!(matches!(
            read_gate(malformed_engagement),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut unexpected_engagement = gate_doc(&SponsorshipGateFence {
            kill_switch_engaged: false,
            ..fence
        })
        .unwrap();
        unexpected_engagement.kill_switch_engagement_id = Some(Uuid::new_v4().to_string());
        assert!(matches!(
            read_gate(unexpected_engagement),
            Err(SponsorshipStoreError::Malformed(_))
        ));
    }

    #[test]
    fn sponsorship_startup_gate_accepts_only_exact_or_single_engagement_transition() {
        let configured = test_gate('a', 7, false);
        let engaged = SponsorshipGateFence {
            epoch: 8,
            kill_switch_engaged: true,
            ..configured.clone()
        };
        assert!(startup_gate_is_compatible(&configured, &configured));
        assert!(startup_gate_is_compatible(&configured, &engaged));

        let configured_engaged = SponsorshipGateFence {
            epoch: 8,
            kill_switch_engaged: true,
            ..configured.clone()
        };
        assert!(startup_gate_is_compatible(
            &configured_engaged,
            &configured_engaged
        ));

        for incompatible in [
            SponsorshipGateFence {
                epoch: 8,
                ..configured.clone()
            },
            SponsorshipGateFence {
                epoch: 9,
                kill_switch_engaged: true,
                ..configured.clone()
            },
            SponsorshipGateFence {
                configuration_digest: "b".repeat(64),
                epoch: 8,
                kill_switch_engaged: true,
                ..configured.clone()
            },
        ] {
            assert!(!startup_gate_is_compatible(&configured, &incompatible));
        }
        assert!(!startup_gate_is_compatible(
            &configured_engaged,
            &configured
        ));

        let exhausted = SponsorshipGateFence {
            epoch: u64::MAX,
            ..configured
        };
        assert!(!startup_gate_is_compatible(
            &exhausted,
            &SponsorshipGateFence {
                kill_switch_engaged: true,
                ..exhausted.clone()
            }
        ));
    }

    #[test]
    fn sponsorship_metadata_read_never_authenticates_credential_secrets() {
        let cipher = test_cipher();
        let operational = test_operational();
        let record = credential_fixture();
        let document = credential_doc(
            &cipher,
            &record,
            &test_provider_identity().encoded_candidates(),
            Some("provider-credential-secret"),
            None,
            0,
        )
        .unwrap();

        let mut raw = mongodb::bson::serialize_to_document(&document).unwrap();
        let current_secret = raw
            .get_document_mut("current_secret")
            .expect("fixture retains current material");
        match current_secret.get_mut("ciphertext") {
            Some(Bson::Binary(ciphertext)) => ciphertext.bytes[0] ^= 0x80,
            _ => panic!("credential ciphertext is stored as BSON binary"),
        }
        let document: CredentialDoc = mongodb::bson::deserialize_from_document(raw).unwrap();

        assert_eq!(
            read_credential_record(&cipher, &operational, &document).unwrap(),
            record
        );
        let error = match read_credential(&cipher, &operational, document) {
            Err(error) => error,
            Ok(_) => panic!("tampered credential material must fail authentication"),
        };
        assert!(matches!(error, SponsorshipStoreError::Malformed(_)));
        assert!(!error.to_string().contains("provider-credential-secret"));
    }

    #[test]
    fn sponsorship_ciphertext_rejects_same_record_public_authority_substitution() {
        let cipher = test_cipher();
        let record = credential_fixture();
        let original = credential_cipher_context(
            &record.id.0,
            &record.owner,
            &record.authority.provider,
            &record.authority.provider_policy_key,
            &record.authority.provider_credential_id_digest,
            &record.authority.ownership_verified_at,
            &record.created_at,
            record.authority.state_version,
            record.authority.supports_expiry,
            record.authority.supports_revocation,
        )
        .unwrap();
        let mut substituted_owner = record.owner.clone();
        match &mut substituted_owner {
            SponsorBillingOwner::Developer { display_name, .. }
            | SponsorBillingOwner::Organization { display_name, .. } => {
                *display_name = "Substituted Owner".into();
            }
        }
        let substituted = credential_cipher_context(
            &record.id.0,
            &substituted_owner,
            &record.authority.provider,
            &record.authority.provider_policy_key,
            &record.authority.provider_credential_id_digest,
            &record.authority.ownership_verified_at,
            &record.created_at,
            record.authority.state_version,
            record.authority.supports_expiry,
            record.authority.supports_revocation,
        )
        .unwrap();
        let envelope = cipher
            .seal(
                "credential",
                &record.id.0,
                "current_secret",
                &original,
                "provider-credential-secret",
            )
            .unwrap();
        assert!(matches!(
            cipher.open(
                "credential",
                &record.id.0,
                "current_secret",
                &substituted,
                &envelope,
            ),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let gate = test_gate('f', 7, false);
        let allocation = SponsoredUsageSummary {
            quota: layer2_core::SponsoredQuota {
                requests: 1,
                input_tokens: 10,
                output_tokens: 10,
            },
            spend_micros: 100,
        };
        let launch_id = LaunchId::new();
        let policy_id = SponsorshipPolicyId::new();
        let original = activation_cipher_context(
            &launch_id.0,
            "session-a",
            &policy_id.0,
            3,
            5,
            &"a".repeat(64),
            &"b".repeat(64),
            &gate,
            &allocation,
            &"c".repeat(64),
            &"d".repeat(64),
            &"e".repeat(64),
            TEST_NOW,
        )
        .unwrap();
        let substituted = activation_cipher_context(
            &launch_id.0,
            "session-a",
            &policy_id.0,
            4,
            5,
            &"a".repeat(64),
            &"b".repeat(64),
            &gate,
            &allocation,
            &"c".repeat(64),
            &"d".repeat(64),
            &"e".repeat(64),
            TEST_NOW,
        )
        .unwrap();
        let envelope = cipher
            .seal(
                "activation",
                "activation-a",
                "provider_grant_id",
                &original,
                "provider-grant-id",
            )
            .unwrap();
        assert!(matches!(
            cipher.open(
                "activation",
                "activation-a",
                "provider_grant_id",
                &substituted,
                &envelope,
            ),
            Err(SponsorshipStoreError::Malformed(_))
        ));
    }

    #[test]
    fn sponsorship_credential_bson_rejects_index_and_schema_drift() {
        let cipher = test_cipher();
        let operational = test_operational();
        let record = credential_fixture();
        let document = credential_doc(
            &cipher,
            &record,
            &test_provider_identity().encoded_candidates(),
            Some("provider-credential-secret"),
            None,
            0,
        )
        .unwrap();
        assert_eq!(
            read_credential(&cipher, &operational, document.clone())
                .unwrap()
                .record,
            record
        );

        let mut unsupported = document.clone();
        unsupported.document_schema_version += 1;
        assert!(matches!(
            read_credential_record(&cipher, &operational, &unsupported),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut wrong_owner_index = document.clone();
        wrong_owner_index.owner_subject = "different-owner".into();
        assert!(matches!(
            read_credential_record(&cipher, &operational, &wrong_owner_index),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut wrong_key_index = document;
        wrong_key_index.encryption_key_id = "unknown-key".into();
        assert!(matches!(
            read_credential_record(&cipher, &operational, &wrong_key_index),
            Err(SponsorshipStoreError::Malformed(_))
        ));
    }

    #[test]
    fn sponsorship_usage_bson_contains_only_opaque_and_keyed_request_identities() {
        const RAW_REQUEST_ID: &str = "12345678";
        let operational = test_operational();
        let reservation =
            usage_reservation_fixture(SponsoredGrantId::new(), SponsoredUsageState::Ambiguous);
        let candidates = operational
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::UsageRequest,
                &[&reservation.grant_id.0, "usage-requester", RAW_REQUEST_ID],
            )
            .unwrap();
        let document = reservation_doc(
            &operational,
            &reservation,
            &candidates.encoded_candidates(),
            &SponsorshipPolicyId::new(),
            DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
        )
        .unwrap();
        let raw = mongodb::bson::serialize_to_document(&document).unwrap();
        assert_eq!(raw.get_str("_id").unwrap(), reservation.id.0);
        assert!(raw.get("stable_request_id").is_none());
        assert!(raw
            .get_document("reservation")
            .unwrap()
            .get("stable_request_id")
            .is_none());
        assert!(!raw.to_string().contains(RAW_REQUEST_ID));
        assert!(!format!("{document:?}").contains("sp1."));
        assert_eq!(
            read_reservation(&operational, document.clone()).unwrap().0,
            reservation
        );

        let mut mismatched_id = document.clone();
        mismatched_id.reservation_id = SponsoredUsageReservationId::new().0;
        assert!(matches!(
            read_reservation(&operational, mismatched_id),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut unknown_alias = document;
        unknown_alias.request_aliases = vec![format!("sp1.unknown.{}", "a".repeat(64))];
        assert!(matches!(
            read_reservation(&operational, unknown_alias),
            Err(SponsorshipStoreError::Malformed(_))
        ));
    }

    #[test]
    fn sponsorship_activation_leases_are_derived_from_database_time() {
        let now = DateTime::from_millis(1_700_000_000_123);
        let holder = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ActivationRequest,
                &["activation-lease-fixture"],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        let (lease, expiry) = activation_lease(holder, now).unwrap();
        assert_eq!(
            expiry.timestamp_millis() - now.timestamp_millis(),
            ACTIVATION_LEASE_MILLIS
        );
        assert_eq!(
            timestamp_seconds(&lease.acquired_at, "lease acquisition").unwrap(),
            database_time_seconds(now).unwrap()
        );
        assert_eq!(
            timestamp_seconds(&lease.expires_at, "lease expiry").unwrap(),
            database_time_seconds(expiry).unwrap()
        );
        assert!(activation_lease("b".repeat(64), now).is_err());
        assert!(activation_lease("not-a-digest".into(), now).is_err());
    }

    #[test]
    fn sponsorship_policy_rate_counter_schema_fails_closed() {
        let valid = PolicyCounterDoc {
            policy_id: SponsorshipPolicyId::new().0,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            rate_revision: 0,
        };
        validate_document_version(
            valid.document_schema_version,
            "sponsorship policy rate counter",
        )
        .unwrap();
        let mut unsupported = valid;
        unsupported.document_schema_version += 1;
        assert!(validate_document_version(
            unsupported.document_schema_version,
            "sponsorship policy rate counter",
        )
        .is_err());
    }

    #[test]
    fn sponsorship_sponsor_counter_and_launch_admission_schemas_fail_closed() {
        let sponsor_key = "a".repeat(64);
        let valid = SponsorCounterDoc {
            sponsor_key: sponsor_key.clone(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            policy_revision: 4,
            launch_revision: 7,
        };
        validate_sponsor_counter(&valid, &sponsor_key).unwrap();

        let mut unsupported = valid.clone();
        unsupported.document_schema_version += 1;
        assert!(validate_sponsor_counter(&unsupported, &sponsor_key).is_err());
        let mut wrong_identity = valid.clone();
        wrong_identity.sponsor_key = "b".repeat(64);
        assert!(validate_sponsor_counter(&wrong_identity, &sponsor_key).is_err());
        let mut negative_policy_revision = valid.clone();
        negative_policy_revision.policy_revision = -1;
        assert!(validate_sponsor_counter(&negative_policy_revision, &sponsor_key).is_err());
        let mut negative_launch_revision = valid;
        negative_launch_revision.launch_revision = -1;
        assert!(validate_sponsor_counter(&negative_launch_revision, &sponsor_key).is_err());

        let launch_id = LaunchId::new();
        let admitted_at_database = DateTime::from_millis(1_700_000_000_123);
        let admission = SponsorLaunchAdmissionDoc {
            launch_id: launch_id.0.clone(),
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            sponsor_key: sponsor_key.clone(),
            admitted_at_database,
        };
        let raw = mongodb::bson::serialize_to_document(&admission).unwrap();
        assert_eq!(raw.get_str("_id").unwrap(), launch_id.0);
        assert_eq!(raw.get_str("sponsor_key").unwrap(), sponsor_key);
        assert_eq!(
            raw.get_datetime("admitted_at_database").unwrap(),
            &admitted_at_database
        );
        assert!(raw.get("admitted_at").is_none());
        validate_document_version(
            admission.document_schema_version,
            "sponsor Launch admission",
        )
        .unwrap();
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_usage_aliases_rotate_recover_and_fail_closed_on_collisions() {
        const RAW_REQUEST_ID: &str = "12345678";
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "cma_usage_alias_{}_{}",
            std::process::id(),
            Uuid::new_v4().simple()
        );
        assert!(database_name.len() <= 63);
        let gate = test_gate('7', 1, false);
        let old_operational_keyring = SponsorshipOperationalKeyring::new(
            "usage-operational-0",
            SponsorshipOperationalKey::from_bytes([0x30; 32]),
            Vec::new(),
        )
        .unwrap();
        let old_operational = old_operational_keyring
            .pseudonymizer("test-deployment")
            .unwrap();
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            old_operational_keyring,
            gate.clone(),
        )
        .await
        .unwrap();
        let reservation =
            usage_reservation_fixture(SponsoredGrantId::new(), SponsoredUsageState::Ambiguous);
        let policy_id = SponsorshipPolicyId::new();
        let old_candidates = old_operational
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::UsageRequest,
                &[&reservation.grant_id.0, "usage-requester", RAW_REQUEST_ID],
            )
            .unwrap();
        store
            .reservations
            .insert_one(
                reservation_doc(
                    &old_operational,
                    &reservation,
                    &old_candidates.encoded_candidates(),
                    &policy_id,
                    DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let raw_reservations = store.reservations.clone_with_type::<Document>();
        let raw = raw_reservations
            .find_one(doc! { "_id": &reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(raw.get_str("_id").unwrap(), reservation.id.0);
        assert!(raw.get("stable_request_id").is_none());
        assert!(raw
            .get_document("reservation")
            .unwrap()
            .get("stable_request_id")
            .is_none());
        assert!(!raw.to_string().contains(RAW_REQUEST_ID));
        assert!(store
            .usage_reservation(&SponsoredUsageReservationId(RAW_REQUEST_ID.into()))
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            store.usage_reservation(&reservation.id).await.unwrap(),
            Some(reservation.clone())
        );
        assert_eq!(
            store
                .usage_recovery_candidates(&SponsorshipRecoveryPageRequest::new(10, None))
                .await
                .unwrap()
                .items,
            vec![reservation.id.clone()]
        );

        let rotated_operational_keyring = SponsorshipOperationalKeyring::new(
            "usage-operational-4",
            SponsorshipOperationalKey::from_bytes([0x34; 32]),
            vec![
                (
                    "usage-operational-3".into(),
                    SponsorshipOperationalKey::from_bytes([0x33; 32]),
                ),
                (
                    "usage-operational-2".into(),
                    SponsorshipOperationalKey::from_bytes([0x32; 32]),
                ),
                (
                    "usage-operational-1".into(),
                    SponsorshipOperationalKey::from_bytes([0x31; 32]),
                ),
                (
                    "usage-operational-0".into(),
                    SponsorshipOperationalKey::from_bytes([0x30; 32]),
                ),
            ],
        )
        .unwrap();
        let rotated_operational = rotated_operational_keyring
            .pseudonymizer("test-deployment")
            .unwrap();
        let rotated_candidates = rotated_operational
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::UsageRequest,
                &[&reservation.grant_id.0, "usage-requester", RAW_REQUEST_ID],
            )
            .unwrap();
        let rotated = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            rotated_operational_keyring,
            gate.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            rotated
                .usage_reservation_for_request(&rotated_candidates)
                .await
                .unwrap(),
            Some(reservation.clone())
        );
        let migrated = rotated
            .reservations
            .find_one(doc! { "_id": &reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            migrated.request_aliases,
            rotated_candidates.encoded_candidates()
        );
        assert_eq!(migrated.request_aliases.len(), 5);

        let raw_gate = rotated.gate.clone_with_type::<Document>();
        let gate_before_refusal = raw_gate.find_one(doc! {}).await.unwrap();
        let missing_predecessors = SponsorshipOperationalKeyring::new(
            "usage-operational-4",
            SponsorshipOperationalKey::from_bytes([0x34; 32]),
            Vec::new(),
        )
        .unwrap();
        assert!(matches!(
            MongoSponsorshipStore::open(
                &client,
                &database_name,
                "test-deployment",
                test_keyring(),
                missing_predecessors,
                gate.clone(),
            )
            .await,
            Err(SponsorshipStoreError::Malformed(_))
        ));
        assert_eq!(
            raw_gate.find_one(doc! {}).await.unwrap(),
            gate_before_refusal
        );

        let collision =
            usage_reservation_fixture(reservation.grant_id.clone(), SponsoredUsageState::Ambiguous);
        let collision_document = reservation_doc(
            &rotated_operational,
            &collision,
            &[rotated_candidates.current().as_str().to_string()],
            &policy_id,
            DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
        )
        .unwrap();
        let collision_error = rotated
            .reservations
            .insert_one(collision_document.clone())
            .await
            .unwrap_err();
        assert!(is_duplicate_key(&collision_error));

        rotated
            .reservations
            .drop_index("sponsorship_usage_by_request_alias")
            .await
            .unwrap();
        rotated
            .reservations
            .insert_one(collision_document)
            .await
            .unwrap();
        assert!(matches!(
            rotated
                .usage_reservation_for_request(&rotated_candidates)
                .await,
            Err(SponsorshipStoreError::Malformed(_))
        ));

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary usage-alias database drops");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_sponsorship_startup_builds_indexed_clock_fenced_grant_recovery() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "cma_sponsor_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        assert!(database_name.len() <= 63);
        let gate = test_gate('c', 1, false);
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            test_operational_keyring(),
            gate.clone(),
        )
        .await
        .expect("replica-set sponsorship store opens");
        assert_eq!(store.gate_fence().await.unwrap(), gate);

        let credential_indexes = store.credentials.list_index_names().await.unwrap();
        assert!(credential_indexes
            .iter()
            .any(|name| name == "sponsorship_credential_provider_identity_alias"));
        assert!(credential_indexes
            .iter()
            .any(|name| name == "sponsorship_credentials_by_owner"));
        let reservation_indexes = store.reservations.list_index_names().await.unwrap();
        assert!(reservation_indexes
            .iter()
            .any(|name| name == "sponsorship_usage_rate_window"));
        let policy_indexes = store.policies.list_index_names().await.unwrap();
        assert!(policy_indexes
            .iter()
            .any(|name| name == "sponsorship_active_policies_by_sponsor"));
        let launch_admission_indexes = store
            .sponsor_launch_admissions
            .list_index_names()
            .await
            .unwrap();
        assert!(launch_admission_indexes
            .iter()
            .any(|name| name == "sponsorship_launch_admissions_by_sponsor"));
        let activation_indexes = store.activations.list_index_names().await.unwrap();
        assert!(activation_indexes
            .iter()
            .any(|name| name == "sponsorship_activations_due"));
        let grant_indexes = store.grants.list_index_names().await.unwrap();
        assert!(grant_indexes
            .iter()
            .any(|name| name == "sponsorship_grants_due"));
        assert!(grant_indexes
            .iter()
            .any(|name| name == "sponsorship_grants_recovery"));

        let (due_active, due_active_receipt) =
            grant_fixture(SponsoredGrantState::Active, "2026-01-02T03:05:00Z");
        let (due_suspended, due_suspended_receipt) =
            grant_fixture(SponsoredGrantState::Suspended, "2026-01-02T03:05:01Z");
        let (future_active, future_active_receipt) =
            grant_fixture(SponsoredGrantState::Active, "2026-01-02T03:06:00Z");
        for (grant, receipt) in [
            (&due_active, &due_active_receipt),
            (&due_suspended, &due_suspended_receipt),
            (&future_active, &future_active_receipt),
        ] {
            store
                .grants
                .insert_one(
                    grant_doc(
                        &store.cipher,
                        &store.operational,
                        grant,
                        receipt,
                        "grant-recovery-sponsor",
                    )
                    .unwrap(),
                )
                .await
                .unwrap();
        }

        let cutoff = "2026-01-02T03:05:30Z";
        let first = store
            .grant_recovery_candidates(cutoff, &SponsorshipRecoveryPageRequest::new(1, None))
            .await
            .unwrap();
        assert_eq!(first.items, vec![due_active.id.clone()]);
        let cursor = first.next.expect("the second due grant continues the page");
        let second = store
            .grant_recovery_candidates(
                cutoff,
                &SponsorshipRecoveryPageRequest::new(1, Some(cursor.clone())),
            )
            .await
            .unwrap();
        assert_eq!(second.items, vec![due_suspended.id.clone()]);
        assert!(second.next.is_none());
        assert!(matches!(
            store
                .grant_recovery_candidates(
                    "2026-01-02T03:06:30Z",
                    &SponsorshipRecoveryPageRequest::new(1, Some(cursor)),
                )
                .await,
            Err(SponsorshipStoreError::Malformed(_))
        ));
        assert!(matches!(
            store
                .grant_recovery_candidates(
                    "not-a-clock-fence",
                    &SponsorshipRecoveryPageRequest::default(),
                )
                .await,
            Err(SponsorshipStoreError::Malformed(_))
        ));
        let later = store
            .grant_recovery_candidates(
                "2026-01-02T03:06:30Z",
                &SponsorshipRecoveryPageRequest::new(10, None),
            )
            .await
            .unwrap();
        assert_eq!(
            later.items,
            vec![due_active.id, due_suspended.id, future_active.id]
        );

        let explain = client
            .database(&database_name)
            .run_command(doc! {
                "explain": {
                    "find": GRANTS,
                    "filter": {
                        "state": "active",
                        "expires_at": { "$lte": cutoff },
                    },
                    "sort": { "expires_at": 1, "_id": 1 },
                    "hint": "sponsorship_grants_due",
                    "limit": 51,
                },
                "verbosity": "executionStats",
            })
            .await
            .unwrap();
        let explain = Bson::Document(explain);
        assert!(bson_tree_has_string(
            &explain,
            "indexName",
            "sponsorship_grants_due"
        ));
        assert!(bson_tree_has_string(&explain, "stage", "IXSCAN"));
        assert!(!bson_tree_has_string(&explain, "stage", "SORT"));

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary sponsorship database drops");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_startup_rejects_an_unconfigured_grant_receipt_key() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "cma_grant_key_{}_{}",
            std::process::id(),
            Uuid::new_v4().simple()
        );
        assert!(database_name.len() <= 63);
        let gate = test_gate('e', 1, false);
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            rotating_test_keyring(),
            test_operational_keyring(),
            gate.clone(),
        )
        .await
        .unwrap();
        let (grant, receipt) = grant_fixture(SponsoredGrantState::Active, "2027-01-02T03:04:05Z");
        store
            .grants
            .insert_one(
                grant_doc(
                    &retiring_test_cipher(),
                    &test_operational(),
                    &grant,
                    &receipt,
                    &grant.sponsor_subject.0,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let startup_error = match MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            rotated_test_keyring(),
            test_operational_keyring(),
            gate,
        )
        .await
        {
            Ok(_) => panic!("startup must reject an unconfigured grant receipt key"),
            Err(error) => error,
        };
        assert!(matches!(
            startup_error,
            SponsorshipStoreError::Unavailable(_)
        ));
        assert!(startup_error.to_string().contains("unavailable key id"));

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary grant-key database drops");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_retired_credential_completes_rejected_cleanup_exactly_once() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "cma_cleanup_{}_{}",
            std::process::id(),
            Uuid::new_v4().simple()
        );
        assert!(database_name.len() <= 63);
        let gate = test_gate('f', 1, false);
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            test_operational_keyring(),
            gate.clone(),
        )
        .await
        .unwrap();

        let policy = policy_fixture();
        let mut credential = credential_fixture();
        credential.id = policy.credential_id.clone();
        credential.owner = policy.owner.clone();
        credential.authority.provider = policy.provider_policy.provider.clone();
        credential.authority.provider_policy_key =
            policy.provider_policy.provider_policy_key.clone();
        let mut session = client.start_session().await.unwrap();
        let now = store.database_now(&mut session).await.unwrap();
        let now_string = database_time_string(now);
        credential.state = SponsoredCredentialState::Revoked;
        credential.material_state = SponsoredCredentialMaterialState::Retired;
        credential.generation = 2;
        credential.updated_at = now_string.clone();
        credential.revoked_at = Some(now_string.clone());
        credential.last_completed_operation = Some(CompletedProviderOperation {
            kind: ProviderOperationKind::RevokeCredential,
            provider_request_id: "c".repeat(64),
            client_request_id_digest: test_operational()
                .pseudonymize(
                    layer2_core::SponsorshipOperationalDomain::CredentialOperationRequest,
                    &["retired-cleanup-fixture"],
                )
                .unwrap()
                .current()
                .as_str()
                .to_string(),
            payload_digest: "d".repeat(64),
            outcome: CompletedProviderOperationOutcome::Confirmed,
            completed_at: now_string.clone(),
        });
        credential.validate().unwrap();

        let (_, receipt) = grant_fixture(SponsoredGrantState::Active, "2027-01-02T03:04:05Z");
        let mut intent = activation_fixture(
            &policy.id,
            &gate,
            GrantActivationState::ProviderAmbiguous,
            &now_string,
            true,
        );
        let holder = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ActivationRequest,
                &[&intent.launch_id.0, "cleanup-holder"],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        let (lease, lease_expiry) = activation_lease(holder.clone(), now).unwrap();
        let cleanup_operation = PendingProviderOperation {
            kind: ProviderOperationKind::RevokeUncommittedGrant,
            phase: ProviderOperationPhase::Rejected,
            provider_request_id: "a".repeat(64),
            client_request_id_digest: test_operational()
                .pseudonymize(
                    layer2_core::SponsorshipOperationalDomain::ActivationRequest,
                    &[&intent.launch_id.0, "cleanup-request"],
                )
                .unwrap()
                .current()
                .as_str()
                .to_string(),
            payload_digest: "b".repeat(64),
            requested_at: lease.acquired_at.clone(),
            attempted_at: Some(lease.acquired_at.clone()),
        };
        intent.state = GrantActivationState::CleanupPending;
        intent.revision = 3;
        intent.lease = Some(lease);
        intent.provider_receipt = Some(receipt);
        intent.cleanup_operation = Some(cleanup_operation.clone());
        intent.updated_at = now_string.clone();
        intent.validate().unwrap();

        let usage = SponsorshipPolicyUsage {
            consumed: SponsoredUsageSummary::default(),
            reserved: intent.allocation.clone(),
            active_launches: 1,
        };
        store
            .policies
            .insert_one(policy_doc(&policy, &usage).unwrap())
            .await
            .unwrap();
        store
            .credentials
            .insert_one(
                credential_doc(
                    &store.cipher,
                    &credential,
                    &test_provider_identity().encoded_candidates(),
                    None,
                    None,
                    0,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .activations
            .insert_one(activation_doc(&store.cipher, &intent, Some(lease_expiry)).unwrap())
            .await
            .unwrap();

        let expected_revision = intent.revision;
        let mut completed = intent.clone();
        completed.revision += 1;
        completed.state = GrantActivationState::Cleaned;
        completed.capacity_held = false;
        completed.lease = None;
        completed.cleanup_operation = None;
        completed.validate().unwrap();
        let audit = SponsoredAuditEvent {
            event_id: Uuid::new_v4().to_string(),
            action: SponsoredAuditAction::UncommittedGrantRevoked,
            actor_digest: "1".repeat(64),
            policy_id: Some(policy.id.clone()),
            grant_id: None,
            stable_request_id_digest: None,
            outcome_code: "provider_authority_revoked".into(),
            occurred_at: completed.updated_at.clone(),
        };
        let command = CompleteGrantActivationCleanup {
            expected_revision,
            holder,
            operation: cleanup_operation,
            global_credential_retirement: true,
            intent: completed.clone(),
            audit,
        };
        let mut missing_signal = command.clone();
        missing_signal.global_credential_retirement = false;
        assert_eq!(
            store
                .complete_grant_activation_cleanup(missing_signal)
                .await,
            Err(SponsorshipStoreError::StaleGeneration)
        );

        assert_eq!(
            store
                .complete_grant_activation_cleanup(command.clone())
                .await
                .unwrap(),
            completed
        );
        assert_eq!(
            store
                .complete_grant_activation_cleanup(command)
                .await
                .unwrap(),
            completed
        );
        assert_eq!(
            store.policy_usage(&policy.id).await.unwrap(),
            SponsorshipPolicyUsage::default()
        );
        assert_eq!(
            store
                .audits
                .count_documents(doc! { "policy_id": &policy.id.0 })
                .await
                .unwrap(),
            1
        );

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary cleanup database drops");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_usage_reconciliation_preserves_reservation_and_grant_time() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "cma_usage_time_{}_{}",
            std::process::id(),
            Uuid::new_v4().simple()
        );
        assert!(database_name.len() <= 63);
        let gate = test_gate('6', 1, false);
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            test_operational_keyring(),
            gate,
        )
        .await
        .unwrap();

        let (grant, receipt) = grant_fixture(SponsoredGrantState::Active, "2027-01-02T03:04:05Z");
        let mut policy = policy_fixture();
        policy.id = grant.policy_id.clone();
        policy.tenant_id = grant.tenant_id.clone();
        policy.owner = SponsorBillingOwner::Developer {
            subject: grant.sponsor_subject.clone(),
            display_name: "Grant Sponsor".into(),
        };
        policy.per_launch_quota = grant.allocation.quota;
        policy.aggregate_quota = grant.allocation.quota;
        policy.per_launch_budget_micros = grant.allocation.spend_micros;
        policy.aggregate_budget_micros = grant.allocation.spend_micros;
        policy.validate().unwrap();
        let policy_usage = SponsorshipPolicyUsage {
            consumed: SponsoredUsageSummary::default(),
            reserved: grant.allocation.clone(),
            active_launches: 1,
        };
        let reservation =
            usage_reservation_fixture(grant.id.clone(), SponsoredUsageState::Ambiguous);
        let request_aliases = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::UsageRequest,
                &[&grant.id.0, "usage-time-fixture"],
            )
            .unwrap();
        let mut grant_document = grant_doc(
            &store.cipher,
            &store.operational,
            &grant,
            &receipt,
            &grant.sponsor_subject.0,
        )
        .unwrap();
        grant_document.unresolved_usage = reservation.reserved.clone();

        store
            .policies
            .insert_one(policy_doc(&policy, &policy_usage).unwrap())
            .await
            .unwrap();
        store.grants.insert_one(grant_document).await.unwrap();
        store
            .reservations
            .insert_one(
                reservation_doc(
                    &store.operational,
                    &reservation,
                    &request_aliases.encoded_candidates(),
                    &policy.id,
                    DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let stale_audit = SponsoredAuditEvent {
            event_id: Uuid::new_v4().to_string(),
            action: SponsoredAuditAction::UsageReconciled,
            actor_digest: "2".repeat(64),
            policy_id: None,
            grant_id: None,
            stable_request_id_digest: None,
            outcome_code: "released".into(),
            occurred_at: "2026-01-02T03:04:04Z".into(),
        };
        assert_eq!(
            store
                .reconcile_usage(
                    ReconcileSponsoredUsage {
                        reservation_id: reservation.id.clone(),
                        request_fingerprint: reservation.request_fingerprint.clone(),
                        actual_quota: None,
                        actual: None,
                        outcome: SponsoredUsageState::Released,
                        observed_at: stale_audit.occurred_at.clone(),
                        dispatch_fence: None,
                        model_response: None,
                    },
                    stale_audit,
                )
                .await,
            Err(SponsorshipStoreError::Conflict)
        );

        let transition_time = "2026-01-02T03:04:06Z";
        let audit = SponsoredAuditEvent {
            event_id: Uuid::new_v4().to_string(),
            action: SponsoredAuditAction::UsageReconciled,
            actor_digest: "2".repeat(64),
            policy_id: None,
            grant_id: None,
            stable_request_id_digest: None,
            outcome_code: "settled".into(),
            occurred_at: transition_time.into(),
        };
        let command = ReconcileSponsoredUsage {
            reservation_id: reservation.id.clone(),
            request_fingerprint: reservation.request_fingerprint.clone(),
            actual_quota: Some(reservation.reserved.quota),
            actual: Some(reservation.reserved.clone()),
            outcome: SponsoredUsageState::Settled,
            observed_at: transition_time.into(),
            dispatch_fence: None,
            model_response: None,
        };
        let settled = store
            .reconcile_usage(command.clone(), audit.clone())
            .await
            .unwrap();
        assert_eq!(settled.updated_at, transition_time);
        assert_eq!(
            store.reconcile_usage(command, audit).await.unwrap(),
            settled
        );
        let stored_grant = store.grant(&grant.id).await.unwrap().unwrap();
        assert_eq!(stored_grant.updated_at, grant.updated_at);
        assert_eq!(stored_grant.usage, reservation.reserved);
        let stored_usage = store.policy_usage(&policy.id).await.unwrap();
        assert_eq!(stored_usage.consumed, reservation.reserved);
        assert_eq!(
            stored_usage.reserved,
            summary_sub(&grant.allocation, &reservation.reserved).unwrap()
        );
        assert_eq!(stored_usage.active_launches, 1);
        assert_eq!(store.audits.count_documents(doc! {}).await.unwrap(), 1);

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary usage-time database drops");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_competing_model_responses_commit_exactly_one_body() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!("cma_mrr_{}_{}", std::process::id(), Uuid::new_v4().simple());
        assert!(database_name.len() <= 63);
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            test_operational_keyring(),
            test_gate('7', 1, false),
        )
        .await
        .unwrap();

        let (grant, receipt) = grant_fixture(SponsoredGrantState::Active, "2027-01-02T03:04:05Z");
        let mut policy = policy_fixture();
        policy.id = grant.policy_id.clone();
        policy.tenant_id = grant.tenant_id.clone();
        policy.owner = SponsorBillingOwner::Developer {
            subject: grant.sponsor_subject.clone(),
            display_name: "Grant Sponsor".into(),
        };
        policy.per_launch_quota = grant.allocation.quota;
        policy.aggregate_quota = grant.allocation.quota;
        policy.per_launch_budget_micros = grant.allocation.spend_micros;
        policy.aggregate_budget_micros = grant.allocation.spend_micros;
        policy.validate().unwrap();
        let policy_usage = SponsorshipPolicyUsage {
            consumed: SponsoredUsageSummary::default(),
            reserved: grant.allocation.clone(),
            active_launches: 1,
        };
        let mut reservation =
            usage_reservation_fixture(grant.id.clone(), SponsoredUsageState::Reserved);
        let holder = "44444444-4444-4444-8444-444444444444".to_string();
        reservation.state = SponsoredUsageState::Attempting;
        reservation.dispatch_revision = 1;
        reservation.dispatch_lease = Some(
            usage_dispatch_lease(
                holder.clone(),
                DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
            )
            .unwrap(),
        );
        reservation.validate().unwrap();
        let request_aliases = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::UsageRequest,
                &[&grant.id.0, "competing-model-response-fixture"],
            )
            .unwrap();
        let request = serde_json::json!({"input": "one provider operation"});
        let dispatch = SponsoredModelDispatchPayload::from_request(
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&request).unwrap())
            ),
            &request,
        )
        .unwrap();
        let mut grant_document = grant_doc(
            &store.cipher,
            &store.operational,
            &grant,
            &receipt,
            &grant.sponsor_subject.0,
        )
        .unwrap();
        grant_document.unresolved_usage = reservation.reserved.clone();
        store
            .policies
            .insert_one(policy_doc(&policy, &policy_usage).unwrap())
            .await
            .unwrap();
        store.grants.insert_one(grant_document).await.unwrap();
        store
            .reservations
            .insert_one(
                model_reservation_doc(
                    &store.cipher,
                    &store.operational,
                    &reservation,
                    &request_aliases.encoded_candidates(),
                    &policy.id,
                    DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
                    &policy.credential_id,
                    &dispatch,
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let response_a = SponsoredModelResponsePayload::from_response(
            dispatch.request_digest().to_string(),
            reservation.reserved.quota,
            &serde_json::json!({"id": "response-a", "object": "response"}),
        )
        .unwrap();
        let response_b = SponsoredModelResponsePayload::from_response(
            dispatch.request_digest().to_string(),
            reservation.reserved.quota,
            &serde_json::json!({"id": "response-b", "object": "response"}),
        )
        .unwrap();
        let command = |response: SponsoredModelResponsePayload| ReconcileSponsoredUsage {
            reservation_id: reservation.id.clone(),
            request_fingerprint: reservation.request_fingerprint.clone(),
            actual_quota: Some(reservation.reserved.quota),
            actual: Some(reservation.reserved.clone()),
            outcome: SponsoredUsageState::Settled,
            observed_at: "2026-01-02T03:04:06Z".into(),
            dispatch_fence: Some(SponsoredUsageReconciliationFence::DispatchClaim {
                revision: 1,
                holder: holder.clone(),
            }),
            model_response: Some(response),
        };
        let audit = || SponsoredAuditEvent {
            event_id: Uuid::new_v4().to_string(),
            action: SponsoredAuditAction::UsageReconciled,
            actor_digest: "2".repeat(64),
            policy_id: Some(policy.id.clone()),
            grant_id: Some(grant.id.clone()),
            stable_request_id_digest: None,
            outcome_code: "settled".into(),
            occurred_at: "2026-01-02T03:04:06Z".into(),
        };
        let (result_a, result_b) = tokio::join!(
            store.reconcile_usage(command(response_a.clone()), audit()),
            store.reconcile_usage(command(response_b.clone()), audit()),
        );
        let a_won = result_a.is_ok();
        assert_ne!(a_won, result_b.is_ok());
        assert!(matches!(
            if a_won { &result_b } else { &result_a },
            Err(SponsorshipStoreError::Conflict)
        ));
        let expected = if a_won { response_a } else { response_b };
        assert_eq!(
            store
                .model_usage_settled_response(&reservation.id)
                .await
                .unwrap(),
            Some(expected)
        );
        let stored_document = store
            .reservations
            .find_one(doc! { "_id": &reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert!(stored_document.model_dispatch_payload.is_none());
        assert!(stored_document.model_response_payload.is_some());
        assert_eq!(store.audits.count_documents(doc! {}).await.unwrap(), 1);

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary model response race database drops");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_ciphertext_rewrap_uses_exact_document_cas_and_preserves_mutations() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "cma_rewrap_{}_{}",
            std::process::id(),
            Uuid::new_v4().simple()
        );
        assert!(database_name.len() <= 63);
        let gate = test_gate('d', 1, false);
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            rotating_test_keyring(),
            rotating_test_operational_keyring(),
            gate.clone(),
        )
        .await
        .unwrap();
        let retiring_cipher = retiring_test_cipher();
        let raw_credentials = store.credentials.clone_with_type::<Document>();
        let raw_activations = store.activations.clone_with_type::<Document>();
        let raw_grants = store.grants.clone_with_type::<Document>();
        let raw_reservations = store.reservations.clone_with_type::<Document>();

        let credential = credential_fixture();
        let mut credential_document = mongodb::bson::serialize_to_document(
            &credential_doc(
                &retiring_cipher,
                &credential,
                &test_provider_identity().encoded_candidates(),
                Some("provider-credential-secret"),
                None,
                0,
            )
            .unwrap(),
        )
        .unwrap();
        credential_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "credential", "nullable": Bson::Null },
        );
        raw_credentials
            .insert_one(credential_document)
            .await
            .unwrap();

        let (mut grant, receipt) =
            grant_fixture(SponsoredGrantState::Active, "2027-01-02T03:04:05Z");
        let mut activation = activation_fixture(
            &SponsorshipPolicyId::new(),
            &gate,
            GrantActivationState::ProviderAmbiguous,
            "2026-01-02T03:04:06Z",
            true,
        );
        activation.provider_receipt = Some(receipt.clone());
        activation.validate().unwrap();
        let mut activation_document = mongodb::bson::serialize_to_document(
            &activation_doc(&retiring_cipher, &activation, None).unwrap(),
        )
        .unwrap();
        activation_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "activation", "nullable": Bson::Null },
        );
        raw_activations
            .insert_one(activation_document)
            .await
            .unwrap();

        let mut grant_document = mongodb::bson::serialize_to_document(
            &grant_doc(
                &retiring_cipher,
                &test_operational(),
                &grant,
                &receipt,
                grant.sponsor_subject.0.as_str(),
            )
            .unwrap(),
        )
        .unwrap();
        grant_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "grant", "nullable": Bson::Null },
        );
        raw_grants.insert_one(grant_document).await.unwrap();

        let request_aliases = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::UsageRequest,
                &[&grant.id.0, "rewrap-request-custody"],
            )
            .unwrap()
            .encoded_candidates();
        let request_reservation =
            usage_reservation_fixture(grant.id.clone(), SponsoredUsageState::Reserved);
        let request_value = serde_json::json!({"input": "retiring request custody"});
        let request_dispatch = SponsoredModelDispatchPayload::from_request(
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&request_value).unwrap())
            ),
            &request_value,
        )
        .unwrap();
        let mut request_document = model_reservation_doc(
            &retiring_cipher,
            &test_operational(),
            &request_reservation,
            &request_aliases,
            &grant.policy_id,
            DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
            &credential.id,
            &request_dispatch,
        )
        .unwrap();
        let mut raw_request_document =
            mongodb::bson::serialize_to_document(&request_document).unwrap();
        raw_request_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "usage_request", "nullable": Bson::Null },
        );
        raw_reservations
            .insert_one(raw_request_document)
            .await
            .unwrap();

        let response_aliases = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::UsageRequest,
                &[&grant.id.0, "rewrap-response-custody"],
            )
            .unwrap()
            .encoded_candidates();
        let mut response_reservation =
            usage_reservation_fixture(grant.id.clone(), SponsoredUsageState::Reserved);
        let response_request_value =
            serde_json::json!({"input": "request replaced by response custody"});
        let response_dispatch = SponsoredModelDispatchPayload::from_request(
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&response_request_value).unwrap())
            ),
            &response_request_value,
        )
        .unwrap();
        let response_request_document = model_reservation_doc(
            &retiring_cipher,
            &test_operational(),
            &response_reservation,
            &response_aliases,
            &grant.policy_id,
            DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
            &credential.id,
            &response_dispatch,
        )
        .unwrap();
        response_reservation.state = SponsoredUsageState::Settled;
        response_reservation.dispatch_revision = 1;
        response_reservation.actual = Some(response_reservation.reserved.clone());
        response_reservation.updated_at = "2026-01-02T03:04:06Z".into();
        response_reservation.validate().unwrap();
        let response_payload = SponsoredModelResponsePayload::from_response(
            response_dispatch.request_digest().to_string(),
            response_reservation.reserved.quota,
            &serde_json::json!({"id": "retiring-response", "object": "response"}),
        )
        .unwrap();
        let response_document = replace_reservation_for_reconciliation(
            &retiring_cipher,
            response_request_document.clone(),
            &response_reservation,
            Some(&response_payload),
        )
        .unwrap();
        let mut raw_response_request_document =
            mongodb::bson::serialize_to_document(&response_request_document).unwrap();
        raw_response_request_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "usage_response_request", "nullable": Bson::Null },
        );
        raw_reservations
            .insert_one(raw_response_request_document)
            .await
            .unwrap();

        let records = [
            (
                SponsorshipCiphertextRecordKind::Credential,
                credential.id.0.clone(),
            ),
            (
                SponsorshipCiphertextRecordKind::GrantActivation,
                activation.launch_id.0.clone(),
            ),
            (SponsorshipCiphertextRecordKind::Grant, grant.id.0.clone()),
            (
                SponsorshipCiphertextRecordKind::UsageReservation,
                request_reservation.id.0.clone(),
            ),
            (
                SponsorshipCiphertextRecordKind::UsageReservation,
                response_reservation.id.0.clone(),
            ),
        ];
        let mut stale_candidates = Vec::new();
        for (kind, record_id) in &records {
            let page = store
                .ciphertext_rewrap_candidates(
                    *kind,
                    "test-retiring",
                    &SponsorshipRecoveryPageRequest::new(10, None),
                )
                .await
                .unwrap();
            assert!(page.next.is_none());
            let candidate = page
                .items
                .into_iter()
                .find(|candidate| candidate.record_id == *record_id)
                .expect("the retiring record is discovered");
            assert!(is_digest(&candidate.document_fingerprint));
            stale_candidates.push(candidate);
        }

        let mut mutated_credential = credential.clone();
        mutated_credential.generation += 1;
        mutated_credential.updated_at = "2026-01-02T03:04:07Z".into();
        mutated_credential.validate().unwrap();
        let mut credential_document = mongodb::bson::serialize_to_document(
            &credential_doc(
                &retiring_cipher,
                &mutated_credential,
                &test_provider_identity().encoded_candidates(),
                Some("provider-credential-secret"),
                None,
                0,
            )
            .unwrap(),
        )
        .unwrap();
        credential_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "credential", "nullable": Bson::Null, "mutation": 2 },
        );
        raw_credentials
            .replace_one(doc! { "_id": &credential.id.0 }, credential_document)
            .await
            .unwrap();

        activation.state = GrantActivationState::ProviderConfirmed;
        activation.revision += 1;
        activation.updated_at = "2026-01-02T03:04:07Z".into();
        activation.validate().unwrap();
        let mut activation_document = mongodb::bson::serialize_to_document(
            &activation_doc(&retiring_cipher, &activation, None).unwrap(),
        )
        .unwrap();
        activation_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "activation", "nullable": Bson::Null, "mutation": 2 },
        );
        raw_activations
            .replace_one(doc! { "_id": &activation.launch_id.0 }, activation_document)
            .await
            .unwrap();

        grant.usage = SponsoredUsageSummary {
            quota: layer2_core::SponsoredQuota {
                requests: 1,
                input_tokens: 50,
                output_tokens: 25,
            },
            spend_micros: 500,
        };
        grant.updated_at = "2026-01-02T03:04:09Z".into();
        grant.validate().unwrap();
        let unresolved_usage = SponsoredUsageSummary {
            quota: layer2_core::SponsoredQuota {
                requests: 1,
                input_tokens: 10,
                output_tokens: 10,
            },
            spend_micros: 100,
        };
        let mut mutated_grant_document = grant_doc(
            &retiring_cipher,
            &test_operational(),
            &grant,
            &receipt,
            grant.sponsor_subject.0.as_str(),
        )
        .unwrap();
        mutated_grant_document.unresolved_usage = unresolved_usage.clone();
        let mut mutated_grant_document =
            mongodb::bson::serialize_to_document(&mutated_grant_document).unwrap();
        mutated_grant_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "grant", "nullable": Bson::Null, "mutation": 2 },
        );
        raw_grants
            .replace_one(doc! { "_id": &grant.id.0 }, mutated_grant_document)
            .await
            .unwrap();

        let mutated_request_value =
            serde_json::json!({"input": "mutated retiring request custody"});
        let mutated_request_dispatch = SponsoredModelDispatchPayload::from_request(
            format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&mutated_request_value).unwrap())
            ),
            &mutated_request_value,
        )
        .unwrap();
        request_document = model_reservation_doc(
            &retiring_cipher,
            &test_operational(),
            &request_reservation,
            &request_aliases,
            &grant.policy_id,
            DateTime::parse_rfc3339_str(TEST_NOW).unwrap(),
            &credential.id,
            &mutated_request_dispatch,
        )
        .unwrap();
        let mut mutated_request_document =
            mongodb::bson::serialize_to_document(&request_document).unwrap();
        mutated_request_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "usage_request", "nullable": Bson::Null, "mutation": 2 },
        );
        raw_reservations
            .replace_one(
                doc! { "_id": &request_reservation.id.0 },
                mutated_request_document,
            )
            .await
            .unwrap();

        let mut settled_response_document =
            mongodb::bson::serialize_to_document(&response_document).unwrap();
        settled_response_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "usage_response", "nullable": Bson::Null, "mutation": "request_to_response" },
        );
        raw_reservations
            .replace_one(
                doc! { "_id": &response_reservation.id.0 },
                settled_response_document,
            )
            .await
            .unwrap();

        let stale_response_request_candidate = stale_candidates
            .iter()
            .find(|candidate| candidate.record_id == response_reservation.id.0)
            .expect("request custody for the future response row was discovered");
        assert!(!store
            .rewrap_ciphertext(stale_response_request_candidate)
            .await
            .unwrap());
        for candidate in stale_candidates
            .iter()
            .filter(|candidate| candidate.record_id != response_reservation.id.0)
        {
            assert!(!store.rewrap_ciphertext(candidate).await.unwrap());
        }
        let stored_credential = store.credential(&credential.id).await.unwrap().unwrap();
        assert_eq!(stored_credential.record, mutated_credential);
        assert_eq!(
            stored_credential.secret.unwrap().expose(),
            "provider-credential-secret"
        );
        assert_eq!(
            store
                .grant_activation(&activation.launch_id)
                .await
                .unwrap()
                .unwrap(),
            activation
        );
        assert_eq!(store.grant(&grant.id).await.unwrap().unwrap(), grant);
        let stored_request_document = store
            .reservations
            .find_one(doc! { "_id": &request_reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            read_model_dispatch(&store.cipher, &stored_request_document).unwrap(),
            Some(mutated_request_dispatch.clone())
        );
        let stored_response_document = store
            .reservations
            .find_one(doc! { "_id": &response_reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert!(stored_response_document.model_dispatch_payload.is_none());
        assert!(stored_response_document.model_response_payload.is_some());
        assert_eq!(
            read_model_response(&store.cipher, &stored_response_document).unwrap(),
            Some(response_payload.clone())
        );
        assert_eq!(
            raw_grants
                .find_one(doc! { "_id": &grant.id.0 })
                .await
                .unwrap()
                .unwrap()
                .get_document("unresolved_usage")
                .unwrap(),
            &mongodb::bson::serialize_to_document(&unresolved_usage).unwrap()
        );

        let stale_response_candidate = store
            .ciphertext_rewrap_candidates(
                SponsorshipCiphertextRecordKind::UsageReservation,
                "test-retiring",
                &SponsorshipRecoveryPageRequest::new(10, None),
            )
            .await
            .unwrap()
            .items
            .into_iter()
            .find(|candidate| candidate.record_id == response_reservation.id.0)
            .expect("settled response custody is rediscovered");
        let mutated_response_payload = SponsoredModelResponsePayload::from_response(
            response_dispatch.request_digest().to_string(),
            response_reservation.reserved.quota,
            &serde_json::json!({"id": "mutated-retiring-response", "object": "response"}),
        )
        .unwrap();
        let mutated_response_document = replace_reservation_for_reconciliation(
            &retiring_cipher,
            response_request_document,
            &response_reservation,
            Some(&mutated_response_payload),
        )
        .unwrap();
        let mut mutated_response_document =
            mongodb::bson::serialize_to_document(&mutated_response_document).unwrap();
        mutated_response_document.insert(
            "rewrap_sentinel",
            doc! { "kind": "usage_response", "nullable": Bson::Null, "mutation": 2 },
        );
        raw_reservations
            .replace_one(
                doc! { "_id": &response_reservation.id.0 },
                mutated_response_document,
            )
            .await
            .unwrap();
        assert!(!store
            .rewrap_ciphertext(&stale_response_candidate)
            .await
            .unwrap());
        let stored_response_document = store
            .reservations
            .find_one(doc! { "_id": &response_reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert!(stored_response_document.model_dispatch_payload.is_none());
        assert!(stored_response_document.model_response_payload.is_some());
        assert_eq!(
            read_model_response(&store.cipher, &stored_response_document).unwrap(),
            Some(mutated_response_payload.clone())
        );

        for (kind, record_id) in &records {
            let candidate = store
                .ciphertext_rewrap_candidates(
                    *kind,
                    "test-retiring",
                    &SponsorshipRecoveryPageRequest::new(10, None),
                )
                .await
                .unwrap()
                .items
                .into_iter()
                .find(|candidate| candidate.record_id == *record_id)
                .expect("the exact retiring record is rediscovered");
            assert_eq!(candidate.record_id, record_id.as_str());
            let collection = match kind {
                SponsorshipCiphertextRecordKind::Credential => raw_credentials.clone(),
                SponsorshipCiphertextRecordKind::GrantActivation => raw_activations.clone(),
                SponsorshipCiphertextRecordKind::Grant => raw_grants.clone(),
                SponsorshipCiphertextRecordKind::UsageReservation => raw_reservations.clone(),
            };
            let before = collection
                .find_one(doc! { "_id": &record_id })
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                rewrap_document_fingerprint(*kind, &before).unwrap(),
                candidate.document_fingerprint
            );
            assert!(store.rewrap_ciphertext(&candidate).await.unwrap());
            let after = collection
                .find_one(doc! { "_id": &record_id })
                .await
                .unwrap()
                .unwrap();
            assert_ne!(before, after);
            assert_eq!(
                rewrap_public_projection(*kind, before),
                rewrap_public_projection(*kind, after.clone())
            );
            assert_eq!(after.get_str("encryption_key_id").unwrap(), "test-current");
            assert!(!store.rewrap_ciphertext(&candidate).await.unwrap());
        }

        let stored_credential = store.credential(&credential.id).await.unwrap().unwrap();
        assert_eq!(stored_credential.record, mutated_credential);
        assert_eq!(
            stored_credential.secret.unwrap().expose(),
            "provider-credential-secret"
        );
        assert_eq!(
            store
                .grant_activation(&activation.launch_id)
                .await
                .unwrap()
                .unwrap(),
            activation
        );
        assert_eq!(store.grant(&grant.id).await.unwrap().unwrap(), grant);
        assert_eq!(
            store.provider_grant_receipt(&grant.id).await.unwrap(),
            Some(receipt.clone())
        );
        for (kind, _) in &records {
            assert!(store
                .ciphertext_rewrap_candidates(
                    *kind,
                    "test-retiring",
                    &SponsorshipRecoveryPageRequest::new(10, None),
                )
                .await
                .unwrap()
                .items
                .is_empty());
        }

        let without_retired_aead = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            rotated_test_keyring(),
            rotating_test_operational_keyring(),
            gate.clone(),
        )
        .await
        .expect("rewrapped records open after the retired AEAD key is removed");
        let reopened_credential = without_retired_aead
            .credential(&credential.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reopened_credential.record, mutated_credential);
        assert_eq!(
            reopened_credential.secret.unwrap().expose(),
            "provider-credential-secret"
        );
        assert_eq!(
            without_retired_aead
                .grant_activation(&activation.launch_id)
                .await
                .unwrap()
                .unwrap(),
            activation
        );
        assert_eq!(
            without_retired_aead
                .grant(&grant.id)
                .await
                .unwrap()
                .unwrap(),
            grant
        );
        assert_eq!(
            without_retired_aead
                .provider_grant_receipt(&grant.id)
                .await
                .unwrap(),
            Some(receipt)
        );
        let reopened_request_document = without_retired_aead
            .reservations
            .find_one(doc! { "_id": &request_reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            read_model_dispatch(&without_retired_aead.cipher, &reopened_request_document).unwrap(),
            Some(mutated_request_dispatch)
        );
        let reopened_response_document = without_retired_aead
            .reservations
            .find_one(doc! { "_id": &response_reservation.id.0 })
            .await
            .unwrap()
            .unwrap();
        assert!(reopened_response_document.model_dispatch_payload.is_none());
        assert!(reopened_response_document.model_response_payload.is_some());
        assert_eq!(
            read_model_response(&without_retired_aead.cipher, &reopened_response_document).unwrap(),
            Some(mutated_response_payload)
        );

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary rewrap database drops");
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_kill_switch_drain_is_bounded_resumable_and_fences_late_awaiting_states() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "cma_drain_{}_{}",
            std::process::id(),
            Uuid::new_v4().simple()
        );
        assert!(database_name.len() <= 63);
        let gate = test_gate('d', 1, false);
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            test_operational_keyring(),
            gate.clone(),
        )
        .await
        .unwrap();
        let mut policy = policy_fixture();
        policy.max_concurrent_launches = 10;
        policy.aggregate_quota = layer2_core::SponsoredQuota {
            requests: 10,
            input_tokens: 100,
            output_tokens: 100,
        };
        policy.aggregate_budget_micros = 1_000;
        policy.validate().unwrap();
        store
            .policies
            .insert_one(policy_doc(&policy, &SponsorshipPolicyUsage::default()).unwrap())
            .await
            .unwrap();
        let held_policy = policy_fixture();
        let held_usage = SponsorshipPolicyUsage {
            consumed: SponsoredUsageSummary::default(),
            reserved: SponsoredUsageSummary {
                quota: layer2_core::SponsoredQuota {
                    requests: 2,
                    input_tokens: 20,
                    output_tokens: 20,
                },
                spend_micros: 200,
            },
            active_launches: 2,
        };
        store
            .policies
            .insert_one(policy_doc(&held_policy, &held_usage).unwrap())
            .await
            .unwrap();

        let activation_count = usize::try_from(MAX_KILL_SWITCH_INVALIDATIONS).unwrap() * 2 + 1;
        let mut documents = Vec::with_capacity(activation_count);
        for index in 0..activation_count {
            let state = if index % 2 == 0 {
                GrantActivationState::AwaitingBilling
            } else {
                GrantActivationState::AwaitingProvider
            };
            let (policy_id, created_at, capacity_held) = if index < 2 {
                (&held_policy.id, "2026-01-02T03:04:05Z", true)
            } else {
                (&policy.id, "2026-01-02T03:04:06Z", false)
            };
            let intent = activation_fixture(policy_id, &gate, state, created_at, capacity_held);
            documents.push(activation_doc(&store.cipher, &intent, None).unwrap());
        }
        store.activations.insert_many(documents).await.unwrap();

        let first_audit = kill_switch_audit("2026-01-02T03:04:07Z");
        let first_engagement_id = first_audit.event_id.clone();
        let first = store.engage_kill_switch(gate.epoch, first_audit).await;
        assert!(matches!(first, Err(SponsorshipStoreError::Unavailable(_))));
        let after_first = store
            .gate
            .find_one(doc! { "_id": GATE_ID })
            .await
            .unwrap()
            .unwrap();
        assert!(after_first.kill_switch_engaged);
        assert!(!after_first.kill_switch_drain_complete);
        assert_eq!(after_first.epoch, 2);
        assert_eq!(
            after_first.kill_switch_engagement_id.as_deref(),
            Some(first_engagement_id.as_str())
        );
        assert_eq!(
            store
                .activations
                .count_documents(doc! {
                    "state": { "$in": ["awaiting_billing", "awaiting_provider"] }
                })
                .await
                .unwrap(),
            u64::try_from(MAX_KILL_SWITCH_INVALIDATIONS + 1).unwrap()
        );
        assert_eq!(
            store.policy_usage(&held_policy.id).await.unwrap(),
            SponsorshipPolicyUsage::default()
        );
        assert_eq!(
            store
                .audits
                .count_documents(doc! { "policy_id": &held_policy.id.0 })
                .await
                .unwrap(),
            2
        );

        let restarted = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            test_operational_keyring(),
            gate.clone(),
        )
        .await
        .unwrap();
        let after_restart = restarted
            .gate
            .find_one(doc! { "_id": GATE_ID })
            .await
            .unwrap()
            .unwrap();
        assert!(!after_restart.kill_switch_drain_complete);
        assert_eq!(after_restart.epoch, 2);
        assert_eq!(
            restarted
                .activations
                .count_documents(doc! {
                    "state": { "$in": ["awaiting_billing", "awaiting_provider"] }
                })
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            restarted.policy_usage(&held_policy.id).await.unwrap(),
            SponsorshipPolicyUsage::default()
        );
        assert_eq!(
            restarted
                .audits
                .count_documents(doc! { "policy_id": &held_policy.id.0 })
                .await
                .unwrap(),
            2
        );

        let completed = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            test_operational_keyring(),
            gate.clone(),
        )
        .await
        .unwrap();
        let completed_gate = completed
            .gate
            .find_one(doc! { "_id": GATE_ID })
            .await
            .unwrap()
            .unwrap();
        assert!(completed_gate.kill_switch_drain_complete);
        assert_eq!(completed_gate.epoch, 2);
        assert_eq!(
            completed_gate.kill_switch_engagement_id.as_deref(),
            Some(first_engagement_id.as_str())
        );
        assert_eq!(
            completed
                .activations
                .count_documents(doc! {
                    "state": { "$in": ["awaiting_billing", "awaiting_provider"] }
                })
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            completed.policy_usage(&held_policy.id).await.unwrap(),
            SponsorshipPolicyUsage::default()
        );
        assert_eq!(
            completed
                .audits
                .count_documents(doc! { "policy_id": &held_policy.id.0 })
                .await
                .unwrap(),
            2
        );

        let replay = completed
            .engage_kill_switch(2, kill_switch_audit("2026-01-02T03:04:08Z"))
            .await
            .expect("an exact bodyless retry observes the completed engagement");
        assert_eq!(replay.epoch, 2);
        assert!(replay.kill_switch_engaged);
        assert_eq!(
            completed.policy_usage(&held_policy.id).await.unwrap(),
            SponsorshipPolicyUsage::default()
        );
        assert_eq!(
            completed
                .audits
                .count_documents(doc! { "policy_id": &held_policy.id.0 })
                .await
                .unwrap(),
            2
        );

        let mut dispatch_intent = activation_fixture(
            &held_policy.id,
            &gate,
            GrantActivationState::AwaitingBilling,
            "2026-01-02T03:04:09Z",
            true,
        );
        let acquired = DateTime::parse_rfc3339_str(&dispatch_intent.created_at).unwrap();
        let dispatch_holder = test_operational()
            .pseudonymize(
                layer2_core::SponsorshipOperationalDomain::ActivationRequest,
                &[&dispatch_intent.launch_id.0, "dispatch-holder-fixture"],
            )
            .unwrap()
            .current()
            .as_str()
            .to_string();
        let (dispatch_lease, dispatch_lease_expiry) =
            activation_lease(dispatch_holder.clone(), acquired).unwrap();
        dispatch_intent.lease = Some(dispatch_lease);
        dispatch_intent.validate().unwrap();
        let dispatch_usage = SponsorshipPolicyUsage {
            consumed: SponsoredUsageSummary::default(),
            reserved: dispatch_intent.allocation.clone(),
            active_launches: 1,
        };
        completed
            .policies
            .replace_one(
                doc! { "_id": &held_policy.id.0 },
                policy_doc(&held_policy, &dispatch_usage).unwrap(),
            )
            .await
            .unwrap();
        completed
            .activations
            .insert_one(
                activation_doc(
                    &completed.cipher,
                    &dispatch_intent,
                    Some(dispatch_lease_expiry),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let mut dispatch_candidate = dispatch_intent.clone();
        dispatch_candidate.state = GrantActivationState::BillingAttempting;
        dispatch_candidate.revision += 1;
        dispatch_candidate.updated_at = "2026-01-02T03:04:10Z".into();
        dispatch_candidate.validate().unwrap();
        assert_eq!(
            completed
                .update_grant_activation(
                    dispatch_intent.revision,
                    dispatch_holder.as_str(),
                    &dispatch_candidate,
                )
                .await
                .unwrap_err(),
            SponsorshipStoreError::GateChanged
        );
        assert_eq!(
            completed
                .grant_activation(&dispatch_intent.launch_id)
                .await
                .unwrap()
                .unwrap()
                .state,
            GrantActivationState::AwaitingBilling
        );
        assert_eq!(
            completed.policy_usage(&held_policy.id).await.unwrap(),
            dispatch_usage
        );

        for (attempting, awaiting) in [
            (
                GrantActivationState::BillingAttempting,
                GrantActivationState::AwaitingBilling,
            ),
            (
                GrantActivationState::ProviderAttempting,
                GrantActivationState::AwaitingProvider,
            ),
        ] {
            let intent =
                activation_fixture(&policy.id, &gate, attempting, "2026-01-02T03:04:09Z", true);
            let lease_expiry =
                DateTime::parse_rfc3339_str(intent.lease.as_ref().unwrap().expires_at.as_str())
                    .unwrap();
            let holder = intent.lease.as_ref().unwrap().holder.clone();
            completed
                .activations
                .insert_one(activation_doc(&completed.cipher, &intent, Some(lease_expiry)).unwrap())
                .await
                .unwrap();
            let mut candidate = intent.clone();
            candidate.state = awaiting;
            candidate.capacity_held = false;
            candidate.lease = None;
            candidate.revision += 1;
            candidate.updated_at = "2026-01-02T03:04:10Z".into();
            candidate.validate().unwrap();
            assert_eq!(
                completed
                    .update_grant_activation(intent.revision, holder.as_str(), &candidate)
                    .await
                    .unwrap_err(),
                SponsorshipStoreError::GateChanged
            );
            assert_eq!(
                completed
                    .grant_activation(&intent.launch_id)
                    .await
                    .unwrap()
                    .unwrap()
                    .state,
                attempting
            );
        }

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary kill-switch database drops");
    }
}
