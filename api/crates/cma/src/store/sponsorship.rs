//! Sponsorship authority, accounting, encrypted provider material, and audit in MongoDB.

use std::sync::Arc;

use futures_util::{future::BoxFuture, FutureExt as _, TryStreamExt as _};
use layer2_core::{
    BeginProviderGrantIssue, ClaimGrantActivation, ClaimGrantActivationCleanup,
    ClaimInvalidatedGrantActivation, CommitGrantActivation, CompleteGrantActivationCleanup,
    CompleteInvalidatedGrantActivation, CompletedProviderOperation,
    CompletedProviderOperationOutcome, GrantActivationClaim, GrantActivationDenial,
    GrantActivationIntent, GrantActivationLease, GrantActivationState, GrantCapacityRelease,
    InertGrantCapability, LaunchId, PendingProviderOperation, PersistSponsoredAcceptance,
    ProviderCredentialAuthority, ProviderCredentialSecret, ProviderGrantAuthority,
    ProviderGrantReceipt, ProviderOperationKind, ProviderOperationPhase, ReconcileSponsoredUsage,
    ReservationDisposition, ReserveSponsoredUsage, SensitiveProviderValue, SessionId,
    SponsorBillingOwner, SponsoredAuditEvent, SponsoredCredentialId,
    SponsoredCredentialMaterialState, SponsoredCredentialRecord, SponsoredCredentialState,
    SponsoredGrant, SponsoredGrantAcceptance, SponsoredGrantId, SponsoredGrantState,
    SponsoredGrantTerms, SponsoredLaunchAuthority, SponsoredLaunchState, SponsoredUsageReservation,
    SponsoredUsageReservationOutcome, SponsoredUsageState, SponsoredUsageSummary,
    SponsorshipGateFence, SponsorshipPolicy, SponsorshipPolicyId, SponsorshipPolicyUsage,
    SponsorshipStore, SponsorshipStoreError, StoredSponsoredCredential, SubjectId,
    TriggerRevisionId, MAX_SPONSORED_AUDIT_PAGE,
};
use mongodb::bson::{doc, Bson, DateTime, Document};
use mongodb::error::{Error, ErrorKind, WriteFailure};
use mongodb::options::{IndexOptions, ReadConcern, WriteConcern};
use mongodb::{Client, ClientSession, Collection, Database, IndexModel};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use super::durability::majority_collection;
use super::sponsorship_encryption::{
    SponsorshipCipher, SponsorshipCipherEnvelope, SponsorshipKeyring,
};

const DOCUMENT_SCHEMA_VERSION: i32 = 1;
const GATE_ID: &str = "sponsorship-gate-v1";
const GATE: &str = "sponsorship_gate";
const CREDENTIALS: &str = "sponsorship_credentials";
const CREDENTIAL_REGISTRATIONS: &str = "sponsorship_credential_registrations";
const CREDENTIAL_OPERATIONS: &str = "sponsorship_credential_operations";
const POLICIES: &str = "sponsorship_policies";
const POLICY_COUNTERS: &str = "sponsorship_policy_counters";
const ACCEPTANCES: &str = "sponsorship_acceptances";
const ACTIVATIONS: &str = "sponsorship_grant_activations";
const GRANTS: &str = "sponsorship_grants";
const RESERVATIONS: &str = "sponsorship_usage_reservations";
const AUDITS: &str = "sponsorship_audit_events";
const TRIGGER_LAUNCHES: &str = "trigger_launches";
const DUPLICATE_KEY: i32 = 11_000;
const ACTIVATION_LEASE_MILLIS: i64 = 30_000;
const RATE_WINDOW_MILLIS: i64 = 60_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GateDoc {
    #[serde(rename = "_id")]
    id: String,
    document_schema_version: i32,
    configuration_digest: String,
    epoch: i64,
    kill_switch_engaged: bool,
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
    registration_idempotency_hash: String,
    document_schema_version: i32,
    credential_id: String,
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
    credential_id: String,
    state: String,
    expires_at: String,
    updated_at: String,
    policy: SponsorshipPolicy,
    usage: PolicyUsageDoc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PolicyCounterDoc {
    #[serde(rename = "_id")]
    policy_id: String,
    document_schema_version: i32,
    rate_revision: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AcceptanceDoc {
    #[serde(rename = "_id")]
    launch_id: String,
    document_schema_version: i32,
    contributor: String,
    policy_id: String,
    accepted_at: String,
    acceptance: SponsoredGrantAcceptance,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GateFenceDoc {
    configuration_digest: String,
    epoch: i64,
    kill_switch_engaged: bool,
}

#[derive(Clone, Serialize, Deserialize)]
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
    terms_digest: String,
    acceptance: SponsoredGrantAcceptance,
    authority: ProviderGrantAuthority,
    capability: InertGrantCapability,
    state: String,
    allocation: layer2_core::SponsoredUsageSummary,
    usage: layer2_core::SponsoredUsageSummary,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReservationDoc {
    #[serde(rename = "_id")]
    stable_request_id: String,
    document_schema_version: i32,
    grant_id: String,
    policy_id: String,
    state: String,
    created_at: String,
    updated_at: String,
    rate_observed_at: DateTime,
    reservation: SponsoredUsageReservation,
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
    gate: Collection<GateDoc>,
    credentials: Collection<CredentialDoc>,
    credential_registrations: Collection<CredentialRegistrationDoc>,
    credential_operations: Collection<CredentialOperationLedgerDoc>,
    policies: Collection<PolicyDoc>,
    policy_counters: Collection<PolicyCounterDoc>,
    acceptances: Collection<AcceptanceDoc>,
    activations: Collection<ActivationDoc>,
    grants: Collection<GrantDoc>,
    reservations: Collection<ReservationDoc>,
    audits: Collection<AuditDoc>,
    trigger_launches: Collection<Document>,
}

impl MongoSponsorshipStore {
    /// Open the sponsorship authority only on a transaction-capable Mongo topology.
    pub(crate) async fn open(
        client: &Client,
        database_name: &str,
        deployment: &str,
        keyring: SponsorshipKeyring,
        initial_gate: SponsorshipGateFence,
    ) -> Result<Self, SponsorshipStoreError> {
        validate_gate(&initial_gate)?;
        let database = client.database(database_name);
        validate_transaction_topology(&database).await?;
        let store = Self {
            client: client.clone(),
            cipher: Arc::new(SponsorshipCipher::new(deployment, keyring)?),
            gate: majority_collection(&database, GATE),
            credentials: majority_collection(&database, CREDENTIALS),
            credential_registrations: majority_collection(&database, CREDENTIAL_REGISTRATIONS),
            credential_operations: majority_collection(&database, CREDENTIAL_OPERATIONS),
            policies: majority_collection(&database, POLICIES),
            policy_counters: majority_collection(&database, POLICY_COUNTERS),
            acceptances: majority_collection(&database, ACCEPTANCES),
            activations: majority_collection(&database, ACTIVATIONS),
            grants: majority_collection(&database, GRANTS),
            reservations: majority_collection(&database, RESERVATIONS),
            audits: majority_collection(&database, AUDITS),
            trigger_launches: majority_collection(&database, TRIGGER_LAUNCHES),
        };
        store.probe_transaction_capability().await?;
        store.ensure_indexes().await?;
        store.initialize_gate(&initial_gate).await?;
        tracing::info!(
            sponsorship_encryption_key_id = store.cipher.current_key_id(),
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
                if stored.configuration_digest != initial.configuration_digest {
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
                .keys(doc! { "provider": 1, "provider_credential_id_digest": 1 })
                .options(unique_named("sponsorship_credential_provider_identity"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "owner_subject": 1, "updated_at": -1, "_id": -1 })
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
        self.credential_registrations
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "credential_id": 1 })
                    .options(named("sponsorship_registration_by_credential"))
                    .build(),
            )
            .await
            .map_err(|error| unavailable("create sponsorship registration index", error))?;
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
        for index in [
            IndexModel::builder()
                .keys(doc! { "owner_subject": 1, "updated_at": -1, "_id": -1 })
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
        ] {
            self.policies
                .create_index(index)
                .await
                .map_err(|error| unavailable("create sponsorship policy index", error))?;
        }
        self.acceptances
            .create_index(
                IndexModel::builder()
                    .keys(doc! { "policy_id": 1, "accepted_at": -1, "_id": -1 })
                    .options(named("sponsorship_acceptances_by_policy"))
                    .build(),
            )
            .await
            .map_err(|error| unavailable("create sponsorship acceptance index", error))?;
        for index in [
            IndexModel::builder()
                .keys(
                    doc! { "state": 1, "lease.expires_at_database": 1, "updated_at": 1, "_id": 1 },
                )
                .options(named("sponsorship_activations_due"))
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
                .keys(doc! { "contributor": 1, "updated_at": -1, "_id": -1 })
                .options(named("sponsorship_grants_by_contributor"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "policy_id": 1, "state": 1, "expires_at": 1, "_id": 1 })
                .options(named("sponsorship_grants_by_policy"))
                .build(),
            IndexModel::builder()
                .keys(doc! { "state": 1, "expires_at": 1, "updated_at": 1, "_id": 1 })
                .options(named("sponsorship_grants_due"))
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
    current_secret: Option<ProviderCredentialSecret>,
    pending_secret: Option<ProviderCredentialSecret>,
}

fn credential_doc(
    cipher: &SponsorshipCipher,
    record: &SponsoredCredentialRecord,
    current_secret: Option<&str>,
    pending_secret: Option<&str>,
) -> Result<CredentialDoc, SponsorshipStoreError> {
    record
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    let record_id = record.id.0.as_str();
    let current_secret = current_secret
        .map(|secret| cipher.seal("credential", record_id, "current_secret", secret))
        .transpose()?;
    let pending_secret = pending_secret
        .map(|secret| cipher.seal("credential", record_id, "pending_secret", secret))
        .transpose()?;
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
            record
                .authority
                .provider_policy_handle
                .expose_for_provider_or_encryption(),
        )?,
        provider_credential_id_digest: record.authority.provider_credential_id_digest.clone(),
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
    })
}

fn read_credential_record(
    cipher: &SponsorshipCipher,
    document: &CredentialDoc,
) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship credential")?;
    let id = SponsoredCredentialId::parse(document.id.clone())
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
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
    document: CredentialDoc,
) -> Result<DecodedCredential, SponsorshipStoreError> {
    let record = read_credential_record(cipher, &document)?;
    let current_secret = document
        .current_secret
        .as_ref()
        .map(|envelope| {
            cipher
                .open("credential", &document.id, "current_secret", envelope)
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
                .open("credential", &document.id, "pending_secret", envelope)
                .and_then(|opened| {
                    ProviderCredentialSecret::new(opened.plaintext.to_string())
                        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))
                })
        })
        .transpose()?;
    Ok(DecodedCredential {
        record,
        current_secret,
        pending_secret,
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
        credential_id: policy.credential_id.0.clone(),
        state: policy_state_code(policy).into(),
        expires_at: policy.expires_at.clone(),
        updated_at: policy.updated_at.clone(),
        policy: policy.clone(),
        usage: PolicyUsageDoc {
            consumed: usage.consumed.clone(),
            reserved: usage.reserved.clone(),
            active_launches: i64::from(usage.active_launches),
        },
    })
}

fn read_policy(
    document: PolicyDoc,
) -> Result<(SponsorshipPolicy, SponsorshipPolicyUsage), SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship policy")?;
    document
        .policy
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if document.id != document.policy.id.0
        || document.owner_subject != document.policy.owner.controller().0
        || document.credential_id != document.policy.credential_id.0
        || document.state != policy_state_code(&document.policy)
        || document.expires_at != document.policy.expires_at
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
    acceptance: &SponsoredGrantAcceptance,
) -> Result<AcceptanceDoc, SponsorshipStoreError> {
    acceptance
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(AcceptanceDoc {
        launch_id: acceptance.launch_id.0.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        contributor: acceptance.contributor.0.clone(),
        policy_id: acceptance.policy_id.0.clone(),
        accepted_at: acceptance.accepted_at.clone(),
        acceptance: acceptance.clone(),
    })
}

fn read_acceptance(
    document: AcceptanceDoc,
) -> Result<SponsoredGrantAcceptance, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship acceptance")?;
    document
        .acceptance
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
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
    })
}

fn read_gate_fence(document: GateFenceDoc) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
    let fence = SponsorshipGateFence {
        configuration_digest: document.configuration_digest,
        epoch: unsigned(document.epoch, "activation gate epoch")?,
        kill_switch_engaged: document.kill_switch_engaged,
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
    receipt: &ProviderGrantReceipt,
) -> Result<ProviderReceiptDoc, SponsorshipStoreError> {
    Ok(ProviderReceiptDoc {
        provider_grant_id: cipher.seal(
            record_kind,
            record_id,
            "provider_grant_id",
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
        &document.provider_grant_id,
    )?;
    let revocation_handle = cipher.open(
        record_kind,
        record_id,
        "revocation_handle",
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
    let lease = match (&intent.lease, lease_expiry_database) {
        (Some(lease), Some(expires_at_database)) => Some(ActivationLeaseDoc {
            holder: lease.holder.clone(),
            acquired_at: lease.acquired_at.clone(),
            expires_at: lease.expires_at.clone(),
            expires_at_database,
        }),
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
            .map(|receipt| provider_receipt_doc(cipher, "activation", &intent.launch_id.0, receipt))
            .transpose()?,
        cleanup_operation: intent.cleanup_operation.as_ref().map(operation_doc),
        created_at: intent.created_at.clone(),
        updated_at: intent.updated_at.clone(),
        encryption_key_id: cipher.current_key_id().into(),
    })
}

fn read_activation(
    cipher: &SponsorshipCipher,
    document: ActivationDoc,
) -> Result<(GrantActivationIntent, Option<DateTime>), SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "grant activation")?;
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
        policy_generation: unsigned(document.policy_generation, "activation policy generation")?,
        credential_generation: unsigned(
            document.credential_generation,
            "activation credential generation",
        )?,
        launch_authority_digest: document.launch_authority_digest,
        acceptance_digest: document.acceptance_digest,
        gate_fence: read_gate_fence(document.gate_fence)?,
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
        revision: unsigned(document.revision, "activation revision")?,
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
    grant: &SponsoredGrant,
    receipt: &ProviderGrantReceipt,
) -> Result<GrantDoc, SponsorshipStoreError> {
    grant
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
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
        terms_digest: grant.terms_digest.clone(),
        acceptance: grant.acceptance.clone(),
        authority: grant.authority.clone(),
        capability: grant.capability.clone(),
        state: grant_state_code(grant.state).into(),
        allocation: grant.allocation.clone(),
        usage: grant.usage.clone(),
        generation: signed(grant.generation, "grant generation")?,
        created_at: grant.created_at.clone(),
        updated_at: grant.updated_at.clone(),
        expires_at: grant.authority.expires_at.clone(),
        revoked_at: grant.revoked_at.clone(),
        pending_revocation: grant.pending_revocation.as_ref().map(operation_doc),
        provider_receipt: provider_receipt_doc(cipher, "grant", &grant.id.0, receipt)?,
        encryption_key_id: cipher.current_key_id().into(),
    })
}

fn read_grant(
    cipher: &SponsorshipCipher,
    document: GrantDoc,
) -> Result<(SponsoredGrant, ProviderGrantReceipt), SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsored grant")?;
    let receipt = read_provider_receipt(
        cipher,
        "grant",
        &document.id,
        document.provider_receipt,
        &document.encryption_key_id,
    )?;
    let grant = SponsoredGrant {
        schema_version: u32::try_from(document.schema_version).map_err(|_| {
            SponsorshipStoreError::Malformed("stored grant schema is invalid".into())
        })?,
        id: SponsoredGrantId::parse(document.id.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        tenant_id: document.tenant_id,
        policy_id: SponsorshipPolicyId::parse(document.policy_id)
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        policy_generation: unsigned(document.policy_generation, "grant policy generation")?,
        credential_generation: unsigned(
            document.credential_generation,
            "grant credential generation",
        )?,
        trigger_revision_id: TriggerRevisionId::parse(document.trigger_revision_id)
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        trigger_revision_digest: document.trigger_revision_digest,
        launch_id: LaunchId::parse(document.launch_id.clone())
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?,
        session_id: SessionId(document.session_id),
        contributor: SubjectId(document.contributor.clone()),
        terms_digest: document.terms_digest,
        acceptance: document.acceptance,
        authority: document.authority,
        capability: document.capability,
        state: parse_grant_state(&document.state)?,
        allocation: document.allocation,
        usage: document.usage,
        generation: unsigned(document.generation, "grant generation")?,
        created_at: document.created_at,
        updated_at: document.updated_at,
        revoked_at: document.revoked_at,
        pending_revocation: document
            .pending_revocation
            .map(read_operation)
            .transpose()?,
    };
    grant
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if document.id != grant.id.0
        || document.launch_id != grant.launch_id.0
        || document.contributor != grant.contributor.0
        || document.expires_at != grant.authority.expires_at
    {
        return Err(SponsorshipStoreError::Malformed(
            "sponsored grant indexes do not match the canonical record".into(),
        ));
    }
    Ok((grant, receipt))
}

fn reservation_doc(
    reservation: &SponsoredUsageReservation,
    policy_id: &SponsorshipPolicyId,
    rate_observed_at: DateTime,
) -> Result<ReservationDoc, SponsorshipStoreError> {
    reservation
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(ReservationDoc {
        stable_request_id: reservation.stable_request_id.clone(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        grant_id: reservation.grant_id.0.clone(),
        policy_id: policy_id.0.clone(),
        state: usage_state_code(reservation.state).into(),
        created_at: reservation.created_at.clone(),
        updated_at: reservation.updated_at.clone(),
        rate_observed_at,
        reservation: reservation.clone(),
    })
}

fn usage_state_code(state: SponsoredUsageState) -> &'static str {
    match state {
        SponsoredUsageState::Reserved => "reserved",
        SponsoredUsageState::Ambiguous => "ambiguous",
        SponsoredUsageState::Settled => "settled",
        SponsoredUsageState::Released => "released",
    }
}

fn read_reservation(
    document: ReservationDoc,
) -> Result<(SponsoredUsageReservation, SponsorshipPolicyId, DateTime), SponsorshipStoreError> {
    validate_document_version(
        document.document_schema_version,
        "sponsored usage reservation",
    )?;
    document
        .reservation
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    if document.stable_request_id != document.reservation.stable_request_id
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

fn credential_registration_bindings_match(
    stored: &SponsoredCredentialRecord,
    candidate: &SponsoredCredentialRecord,
) -> bool {
    stored.id == candidate.id
        && stored.owner == candidate.owner
        && stored.authority.provider == candidate.authority.provider
        && stored.authority.provider_policy_key == candidate.authority.provider_policy_key
        && stored.authority.provider_credential_id_digest
            == candidate.authority.provider_credential_id_digest
        && stored.authority.supports_expiry == candidate.authority.supports_expiry
        && stored.authority.supports_revocation == candidate.authority.supports_revocation
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
    if pending.kind != kind
        || completed.kind != kind
        || completed.outcome != outcome
        || pending.provider_request_id != completed.provider_request_id
        || pending.client_request_id_digest != completed.client_request_id_digest
        || pending.payload_digest != completed.payload_digest
        || current.id != candidate.id
    {
        return Err(SponsorshipStoreError::Conflict);
    }
    completed
        .validate()
        .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
    Ok(completed.clone())
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

fn acceptance_stable_fields_match(
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
        && stored.idempotency_key_hash == candidate.idempotency_key_hash
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

fn activation_lease(
    holder: String,
    now: DateTime,
) -> Result<(GrantActivationLease, DateTime), SponsorshipStoreError> {
    if !is_digest(&holder) {
        return Err(SponsorshipStoreError::Malformed(
            "grant activation lease holder is not a digest".into(),
        ));
    }
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

fn grant_capacity_is_released(state: SponsoredGrantState) -> bool {
    matches!(
        state,
        SponsoredGrantState::Exhausted
            | SponsoredGrantState::Expired
            | SponsoredGrantState::Revoked
            | SponsoredGrantState::Closed
    )
}

fn sensitive_digest(domain: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0]);
    hasher.update((value.len() as u64).to_be_bytes());
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn require_receipt_matches_grant(
    receipt: &ProviderGrantReceipt,
    grant: &SponsoredGrant,
) -> Result<(), SponsorshipStoreError> {
    let provider_grant_id_digest = sensitive_digest(
        "cma-provider-grant-id-v1",
        receipt
            .provider_grant_id
            .expose_for_provider_or_encryption(),
    );
    let revocation_handle_digest = sensitive_digest(
        "cma-provider-revocation-handle-v1",
        receipt
            .revocation_handle
            .expose_for_provider_or_encryption(),
    );
    if grant.authority.provider_grant_id_digest != provider_grant_id_digest
        || grant.authority.revocation_handle_digest != revocation_handle_digest
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
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
        kind: ProviderOperationKind,
        outcome: CompletedProviderOperationOutcome,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
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
                record.clone(),
                audit,
                kind,
                outcome,
            ),
            |session, (store, expected, candidate, audit, kind, outcome)| {
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
                    if current.record.material_state != SponsoredCredentialMaterialState::Retained {
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
                            if store
                                .credentials
                                .find_one(doc! {
                                    "_id": { "$ne": &candidate.id.0 },
                                    "provider": &candidate.authority.provider,
                                    "provider_credential_id_digest":
                                        &candidate.authority.provider_credential_id_digest,
                                })
                                .session(&mut *session)
                                .await?
                                .is_some()
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
                            (None, None)
                        }
                        _ => return Err(custom(SponsorshipStoreError::Conflict)),
                    };
                    let replacement =
                        credential_doc(&store.cipher, candidate, current_secret, pending_secret)
                            .map_err(custom)?;
                    let result = store
                        .credentials
                        .replace_one(
                            doc! {
                                "_id": &candidate.id.0,
                                "generation": signed(*expected, "credential generation")
                                    .map_err(custom)?,
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
            .map(|document| read_activation(&self.cipher, document).map_err(custom))
            .transpose()
    }

    async fn grant_in_transaction(
        &self,
        session: &mut ClientSession,
        grant_id: &SponsoredGrantId,
    ) -> mongodb::error::Result<Option<(SponsoredGrant, ProviderGrantReceipt)>> {
        self.grants
            .find_one(doc! { "_id": &grant_id.0 })
            .session(session)
            .await?
            .map(|document| read_grant(&self.cipher, document).map_err(custom))
            .transpose()
    }

    async fn reservation_in_transaction(
        &self,
        session: &mut ClientSession,
        stable_request_id: &str,
    ) -> mongodb::error::Result<Option<(SponsoredUsageReservation, SponsorshipPolicyId, DateTime)>>
    {
        self.reservations
            .find_one(doc! { "_id": stable_request_id })
            .session(session)
            .await?
            .map(|document| read_reservation(document).map_err(custom))
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

    async fn replace_grant_in_transaction(
        &self,
        session: &mut ClientSession,
        expected_generation: u64,
        grant: &SponsoredGrant,
        receipt: &ProviderGrantReceipt,
    ) -> mongodb::error::Result<()> {
        let replacement = grant_doc(&self.cipher, grant, receipt).map_err(custom)?;
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

    async fn require_activation_dependencies_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
        launch: &SponsoredLaunchAuthority,
        acceptance: &SponsoredGrantAcceptance,
        now: &str,
    ) -> mongodb::error::Result<()> {
        self.require_gate_in_transaction(session, &intent.gate_fence)
            .await?;
        let current_launch = self
            .launch_authority_in_transaction(session, &launch.launch_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
        if current_launch != *launch
            || launch.state != SponsoredLaunchState::Starting
            || launch.session_id.is_none()
            || launch.session_id.as_ref() != Some(&intent.session_id)
            || launch.digest != intent.launch_authority_digest
        {
            return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
        }
        let stored_acceptance = self
            .acceptances
            .find_one(doc! { "_id": &launch.launch_id.0 })
            .session(&mut *session)
            .await?
            .map(read_acceptance)
            .transpose()
            .map_err(custom)?
            .ok_or_else(|| custom(SponsorshipStoreError::AcceptanceChanged))?;
        if stored_acceptance != *acceptance
            || acceptance.digest != intent.acceptance_digest
            || acceptance.launch_id != launch.launch_id
            || acceptance.contributor != launch.contributor
            || acceptance.revision_id != launch.revision_id
            || acceptance.revision_digest != launch.revision_digest
            || acceptance.policy_id != launch.policy_id
            || acceptance.launch_snapshot_digest != launch.launch_snapshot_digest
        {
            return Err(custom(SponsorshipStoreError::AcceptanceChanged));
        }
        let (policy, _) = self
            .policy_in_transaction(session, &intent.policy_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
        let credential = self
            .credential_in_transaction(session, &policy.credential_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
        if !policy.is_available_at(now)
            || policy.id != launch.policy_id
            || policy.tenant_id != launch.tenant_id
            || policy.generation != intent.policy_generation
            || credential.record.state != SponsoredCredentialState::Active
            || credential.record.material_state != SponsoredCredentialMaterialState::Retained
            || credential.current_secret.is_none()
            || credential.record.generation != intent.credential_generation
            || credential.record.owner != policy.owner
            || credential.record.authority.provider != policy.provider_policy.provider
            || credential.record.authority.provider_policy_key
                != policy.provider_policy.provider_policy_key
            || intent.allocation.quota != policy.per_launch_quota
            || intent.allocation.spend_micros != policy.per_launch_budget_micros
        {
            return Err(custom(SponsorshipStoreError::GrantUnavailable));
        }
        Ok(())
    }

    async fn activation_dependencies_are_current_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
        now: DateTime,
    ) -> mongodb::error::Result<bool> {
        let Some(launch) = self
            .launch_authority_in_transaction(session, &intent.launch_id)
            .await?
        else {
            return Ok(false);
        };
        let Some(acceptance) = self
            .acceptances
            .find_one(doc! { "_id": &intent.launch_id.0 })
            .session(&mut *session)
            .await?
            .map(read_acceptance)
            .transpose()
            .map_err(custom)?
        else {
            return Ok(false);
        };
        let now_string = database_time_string(now);
        if let Err(error) = self
            .require_activation_dependencies_in_transaction(
                session,
                intent,
                &launch,
                &acceptance,
                &now_string,
            )
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

    async fn require_current_terms_in_transaction(
        &self,
        session: &mut ClientSession,
        intent: &GrantActivationIntent,
        acceptance: &SponsoredGrantAcceptance,
    ) -> mongodb::error::Result<()> {
        let (policy, usage) = self
            .policy_in_transaction(session, &intent.policy_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
        let terms = SponsoredGrantTerms::from_policy(&policy, &usage)
            .map_err(|error| custom(SponsorshipStoreError::Malformed(error.to_string())))?;
        if acceptance.policy_generation != terms.policy_generation
            || acceptance.terms_digest != terms.digest
        {
            return Err(custom(SponsorshipStoreError::AcceptanceChanged));
        }
        Ok(())
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
        usage.reserved = summary_sub(&usage.reserved, &intent.allocation).map_err(custom)?;
        usage.active_launches = usage
            .active_launches
            .checked_sub(1)
            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
        self.replace_policy_usage_in_transaction(session, &policy, &usage)
            .await
    }

    async fn unresolved_usage_for_grant_in_transaction(
        &self,
        session: &mut ClientSession,
        grant: &SponsoredGrant,
        excluded_request_id: Option<&str>,
    ) -> mongodb::error::Result<SponsoredUsageSummary> {
        let mut filter = doc! {
            "grant_id": &grant.id.0,
            "state": { "$in": ["reserved", "ambiguous"] },
        };
        if let Some(excluded) = excluded_request_id {
            filter.insert("_id", doc! { "$ne": excluded });
        }
        let mut cursor = self
            .reservations
            .find(filter)
            .session(&mut *session)
            .await?;
        let mut held = SponsoredUsageSummary::default();
        while let Some(document) = cursor.next(&mut *session).await.transpose()? {
            let (reservation, policy_id, _) = read_reservation(document).map_err(custom)?;
            if reservation.grant_id != grant.id || policy_id != grant.policy_id {
                return Err(custom(SponsorshipStoreError::Malformed(
                    "usage reservation authority indexes are inconsistent".into(),
                )));
            }
            held = summary_add(&held, &reservation.reserved).map_err(custom)?;
        }
        Ok(held)
    }

    async fn release_terminal_grant_capacity_in_transaction(
        &self,
        session: &mut ClientSession,
        grant: &SponsoredGrant,
        excluded_request_id: Option<&str>,
    ) -> mongodb::error::Result<()> {
        let unresolved = self
            .unresolved_usage_for_grant_in_transaction(session, grant, excluded_request_id)
            .await?;
        let unused = summary_sub(&grant.allocation, &grant.usage).map_err(custom)?;
        let releasable = summary_sub(&unused, &unresolved).map_err(custom)?;
        let (policy, mut usage) = self
            .policy_in_transaction(session, &grant.policy_id)
            .await?
            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
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
            .find_one(doc! { "_id": &id.0 })
            .session(session)
            .await?
            .map(|document| read_credential(&self.cipher, document).map_err(custom))
            .transpose()
    }

    async fn credential_record_in_transaction(
        &self,
        session: &mut ClientSession,
        id: &SponsoredCredentialId,
    ) -> mongodb::error::Result<Option<SponsoredCredentialRecord>> {
        self.credentials
            .find_one(doc! { "_id": &id.0 })
            .session(session)
            .await?
            .map(|document| read_credential_record(&self.cipher, &document).map_err(custom))
            .transpose()
    }

    async fn policy_in_transaction(
        &self,
        session: &mut ClientSession,
        id: &SponsorshipPolicyId,
    ) -> mongodb::error::Result<Option<(SponsorshipPolicy, SponsorshipPolicyUsage)>> {
        self.policies
            .find_one(doc! { "_id": &id.0 })
            .session(session)
            .await?
            .map(|document| read_policy(document).map_err(custom))
            .transpose()
    }

    async fn gate_in_transaction(
        &self,
        session: &mut ClientSession,
    ) -> mongodb::error::Result<SponsorshipGateFence> {
        let document = self
            .gate
            .find_one(doc! { "_id": GATE_ID })
            .session(session)
            .await?
            .ok_or_else(|| {
                custom(SponsorshipStoreError::Malformed(
                    "the sponsorship gate is missing".into(),
                ))
            })?;
        read_gate(document).map_err(custom)
    }

    async fn require_gate_in_transaction(
        &self,
        session: &mut ClientSession,
        expected: &SponsorshipGateFence,
    ) -> mongodb::error::Result<()> {
        let current = self.gate_in_transaction(session).await?;
        if &current != expected || current.kill_switch_engaged {
            return Err(custom(SponsorshipStoreError::GateChanged));
        }
        Ok(())
    }

    async fn launch_authority_in_transaction(
        &self,
        session: &mut ClientSession,
        id: &LaunchId,
    ) -> mongodb::error::Result<Option<SponsoredLaunchAuthority>> {
        let document = self
            .trigger_launches
            .find_one(doc! { "_id": &id.0 })
            .session(session)
            .await?;
        document
            .map(decode_launch_authority)
            .transpose()
            .map_err(custom)
    }
}

/// Kept as one replacement point for the typed Trigger/sponsorship integration.
///
/// The merged Trigger model does not yet contain the tenant/policy binding or a
/// sponsorship-aware effective snapshot. Inventing those values in persistence
/// would weaken the Launch CAS, so a present Launch fails closed until the core
/// mapping can call `SponsoredLaunchAuthority::from_launch`.
fn decode_launch_authority(
    _document: Document,
) -> Result<SponsoredLaunchAuthority, SponsorshipStoreError> {
    Err(SponsorshipStoreError::Unavailable(
        "the durable Launch does not yet expose the typed sponsorship authority mapping".into(),
    ))
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
        self.transaction((self.clone(), audit), |session, (store, audit)| {
            async move {
                let mut gate = store.gate_in_transaction(session).await?;
                if gate.epoch != expected_epoch {
                    return Err(custom(SponsorshipStoreError::GateChanged));
                }
                let now = store.database_now(session).await?;
                let now_string = database_time_string(now);
                let mut cursor = store
                    .activations
                    .find(doc! {
                        "state": { "$in": ["awaiting_billing", "awaiting_provider"] }
                    })
                    .session(&mut *session)
                    .await?;
                let mut terminalizable = Vec::new();
                while let Some(document) = cursor.next(&mut *session).await.transpose()? {
                    terminalizable.push(read_activation(&store.cipher, document).map_err(custom)?);
                }
                for (mut intent, _) in terminalizable {
                    if intent.capacity_held {
                        let (policy, mut usage) = store
                            .policy_in_transaction(session, &intent.policy_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                        usage.reserved =
                            summary_sub(&usage.reserved, &intent.allocation).map_err(custom)?;
                        usage.active_launches = usage
                            .active_launches
                            .checked_sub(1)
                            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                        let replacement = policy_doc(&policy, &usage).map_err(custom)?;
                        store
                            .policies
                            .replace_one(doc! { "_id": &policy.id.0 }, replacement)
                            .session(&mut *session)
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
                    intent.updated_at = now_string.clone();
                    let replacement =
                        activation_doc(&store.cipher, &intent, None).map_err(custom)?;
                    let result = store
                        .activations
                        .replace_one(
                            doc! {
                                "_id": &intent.launch_id.0,
                                "revision": signed(expected_revision, "activation revision")
                                    .map_err(custom)?,
                            },
                            replacement,
                        )
                        .session(&mut *session)
                        .await?;
                    if result.modified_count != 1 {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                }
                gate.epoch = gate.epoch.checked_add(1).ok_or_else(|| {
                    custom(SponsorshipStoreError::Malformed(
                        "sponsorship gate epoch exhausted".into(),
                    ))
                })?;
                gate.kill_switch_engaged = true;
                let result = store
                    .gate
                    .replace_one(
                        doc! {
                            "_id": GATE_ID,
                            "epoch": signed(expected_epoch, "sponsorship gate epoch")
                                .map_err(custom)?,
                        },
                        gate_doc(&gate).map_err(custom)?,
                    )
                    .session(&mut *session)
                    .await?;
                if result.modified_count != 1 {
                    return Err(custom(SponsorshipStoreError::GateChanged));
                }
                store.audit_in_transaction(session, audit).await?;
                Ok(gate)
            }
            .boxed()
        })
        .await
    }

    async fn launch_authority(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredLaunchAuthority>, SponsorshipStoreError> {
        let document = self
            .trigger_launches
            .find_one(doc! { "_id": &launch_id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored Launch authority", error))?;
        document.map(decode_launch_authority).transpose()
    }

    async fn insert_credential(
        &self,
        registration_idempotency_hash: &str,
        record: &SponsoredCredentialRecord,
        secret: &ProviderCredentialSecret,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        record
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        if !is_digest(registration_idempotency_hash)
            || record.material_state != SponsoredCredentialMaterialState::Retained
        {
            return Err(SponsorshipStoreError::Malformed(
                "credential registration identity or material state is invalid".into(),
            ));
        }
        let secret = Zeroizing::new(secret.expose().to_string());
        for attempt in 0..2 {
            let result = self
                .transaction(
                    (
                        self.clone(),
                        registration_idempotency_hash.to_string(),
                        record.clone(),
                        secret.clone(),
                        audit.clone(),
                    ),
                    |session, (store, registration_hash, candidate, secret, audit)| {
                        async move {
                            if let Some(binding) = store
                                .credential_registrations
                                .find_one(doc! { "_id": &*registration_hash })
                                .session(&mut *session)
                                .await?
                            {
                                validate_document_version(
                                    binding.document_schema_version,
                                    "credential registration",
                                )
                                .map_err(custom)?;
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
                                if credential_registration_bindings_match(&stored.record, candidate)
                                {
                                    return Ok(stored.record);
                                }
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            if let Some(document) = store
                                .credentials
                                .find_one(doc! {
                                    "provider": &candidate.authority.provider,
                                    "provider_credential_id_digest":
                                        &candidate.authority.provider_credential_id_digest,
                                })
                                .session(&mut *session)
                                .await?
                            {
                                let stored =
                                    read_credential(&store.cipher, document).map_err(custom)?;
                                if !credential_registration_bindings_match(
                                    &stored.record,
                                    candidate,
                                ) {
                                    return Err(custom(SponsorshipStoreError::Conflict));
                                }
                                store
                                    .credential_registrations
                                    .insert_one(CredentialRegistrationDoc {
                                        registration_idempotency_hash: registration_hash.clone(),
                                        document_schema_version: DOCUMENT_SCHEMA_VERSION,
                                        credential_id: stored.record.id.0.clone(),
                                        created_at: candidate.created_at.clone(),
                                    })
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
                                    store
                                        .credential_registrations
                                        .insert_one(CredentialRegistrationDoc {
                                            registration_idempotency_hash: registration_hash
                                                .clone(),
                                            document_schema_version: DOCUMENT_SCHEMA_VERSION,
                                            credential_id: candidate.id.0.clone(),
                                            created_at: candidate.created_at.clone(),
                                        })
                                        .session(&mut *session)
                                        .await?;
                                    return Ok(stored.record);
                                }
                                return Err(custom(SponsorshipStoreError::Conflict));
                            }
                            let document = credential_doc(
                                &store.cipher,
                                candidate,
                                Some(secret.as_str()),
                                None,
                            )
                            .map_err(custom)?;
                            store
                                .credentials
                                .insert_one(document)
                                .session(&mut *session)
                                .await?;
                            store
                                .credential_registrations
                                .insert_one(CredentialRegistrationDoc {
                                    registration_idempotency_hash: registration_hash.clone(),
                                    document_schema_version: DOCUMENT_SCHEMA_VERSION,
                                    credential_id: candidate.id.0.clone(),
                                    created_at: candidate.created_at.clone(),
                                })
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
            .map(|document| read_credential_record(&self.cipher, document))
            .transpose()
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
                let decoded = read_credential(&self.cipher, document)?;
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
        client_request_id_digest: &str,
    ) -> Result<Option<CompletedProviderOperation>, SponsorshipStoreError> {
        let document = self
            .credential_operations
            .find_one(doc! {
                "_id.credential_id": &id.0,
                "_id.operation_kind": operation_kind_code(kind),
                "_id.client_request_id_digest": client_request_id_digest,
            })
            .await
            .map_err(|error| {
                stored_read_failure("read sponsored credential operation replay", error)
            })?;
        document
            .map(|document| {
                if document.id.credential_id != id.0 {
                    return Err(SponsorshipStoreError::Malformed(
                        "credential operation replay identity is inconsistent".into(),
                    ));
                }
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
                    if candidate.material_state != current.record.material_state
                        || candidate.material_state != SponsoredCredentialMaterialState::Retained
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if current.record.generation != *expected {
                        return if current.record == *candidate {
                            Ok(current.record)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
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
                        current_secret,
                        current
                            .pending_secret
                            .as_ref()
                            .map(|secret| secret.expose()),
                    )
                    .map_err(custom)?;
                    let result = store
                        .credentials
                        .replace_one(
                            doc! {
                                "_id": &candidate.id.0,
                                "generation": signed(*expected, "credential generation")
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
                    if current.record.material_state != SponsoredCredentialMaterialState::Retained {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if current.record.generation != *expected {
                        return if current.record == *candidate {
                            Ok(current.record)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
                    }
                    let current_secret = current.current_secret.as_ref().ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "current credential secret is missing".into(),
                        ))
                    })?;
                    let replacement_doc = credential_doc(
                        &store.cipher,
                        candidate,
                        Some(current_secret.expose()),
                        Some(replacement.as_str()),
                    )
                    .map_err(custom)?;
                    let result = store
                        .credentials
                        .replace_one(
                            doc! {
                                "_id": &candidate.id.0,
                                "generation": signed(*expected, "credential generation")
                                    .map_err(custom)?,
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
                read_credential(&self.cipher, document).map(|decoded| decoded.pending_secret)
            })
            .transpose()
            .map(Option::flatten)
    }

    async fn complete_credential_rotation(
        &self,
        expected_generation: u64,
        record: &SponsoredCredentialRecord,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredCredentialRecord, SponsorshipStoreError> {
        self.complete_credential_mutation(
            expected_generation,
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
            record,
            audit,
            ProviderOperationKind::RevokeCredential,
            CompletedProviderOperationOutcome::Confirmed,
        )
        .await
    }

    async fn insert_policy(
        &self,
        policy: &SponsorshipPolicy,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsorshipPolicy, SponsorshipStoreError> {
        policy
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_audit_event(&audit)?;
        let usage = SponsorshipPolicyUsage {
            consumed: Default::default(),
            reserved: Default::default(),
            active_launches: 0,
        };
        let candidate = policy_doc(policy, &usage)?;
        self.transaction(
            (self.clone(), candidate, policy.clone(), audit),
            |session, (store, candidate, policy, audit)| {
                async move {
                    if let Some(stored) = store
                        .policies
                        .find_one(doc! { "_id": &policy.id.0 })
                        .session(&mut *session)
                        .await?
                    {
                        let (stored, _) = read_policy(stored).map_err(custom)?;
                        if stored != *policy {
                            return Err(custom(SponsorshipStoreError::Conflict));
                        }
                        let counter = store
                            .policy_counters
                            .find_one(doc! { "_id": &policy.id.0 })
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
                        if counter.policy_id != policy.id.0 || counter.rate_revision < 0 {
                            return Err(custom(SponsorshipStoreError::Malformed(
                                "sponsorship policy rate counter is invalid".into(),
                            )));
                        }
                        return Ok(stored);
                    }
                    store
                        .policies
                        .insert_one(candidate.clone())
                        .session(&mut *session)
                        .await?;
                    store
                        .policy_counters
                        .insert_one(PolicyCounterDoc {
                            policy_id: policy.id.0.clone(),
                            document_schema_version: DOCUMENT_SCHEMA_VERSION,
                            rate_revision: 0,
                        })
                        .session(&mut *session)
                        .await?;
                    store.audit_in_transaction(session, audit).await?;
                    Ok(policy.clone())
                }
                .boxed()
            },
        )
        .await
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
        self.transaction(
            (self.clone(), expected_generation, policy.clone(), audit),
            |session, (store, expected, candidate, audit)| {
                async move {
                    let (current, usage) = store
                        .policy_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                    if current.state == layer2_core::SponsorshipPolicyState::Revoked {
                        return Ok(current);
                    }
                    if current.generation != *expected {
                        return if current == *candidate {
                            Ok(current)
                        } else {
                            Err(custom(SponsorshipStoreError::StaleGeneration))
                        };
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
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                if let Some(stored) = store
                    .acceptances
                    .find_one(doc! { "_id": &command.acceptance.launch_id.0 })
                    .session(&mut *session)
                    .await?
                {
                    let stored = read_acceptance(stored).map_err(custom)?;
                    if stored.contributor != command.acceptance.contributor {
                        return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                    }
                    if stored.idempotency_key_hash == command.acceptance.idempotency_key_hash {
                        return if acceptance_stable_fields_match(&stored, &command.acceptance) {
                            Ok(stored)
                        } else {
                            Err(custom(SponsorshipStoreError::Conflict))
                        };
                    }
                }
                store
                    .require_gate_in_transaction(session, &command.gate_fence)
                    .await?;
                let current_launch = store
                    .launch_authority_in_transaction(session, &command.launch_authority.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
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
                store
                    .acceptances
                    .replace_one(
                        doc! { "_id": &command.acceptance.launch_id.0 },
                        acceptance_doc(&command.acceptance).map_err(custom)?,
                    )
                    .upsert(true)
                    .session(&mut *session)
                    .await?;
                store.audit_in_transaction(session, &command.audit).await?;
                Ok(command.acceptance.clone())
            }
            .boxed()
        })
        .await
    }

    async fn acceptance_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrantAcceptance>, SponsorshipStoreError> {
        self.acceptances
            .find_one(doc! { "_id": &launch_id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored acceptance", error))?
            .map(read_acceptance)
            .transpose()
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
        if !is_digest(&command.holder) {
            return Err(SponsorshipStoreError::Malformed(
                "activation lease holder is not a digest".into(),
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
                if let Some((mut existing, lease_expiry)) = store
                    .activation_in_transaction(session, &command.candidate.launch_id)
                    .await?
                {
                    if existing.request_fingerprint != command.candidate.request_fingerprint
                        || existing.billing_request_id != command.candidate.billing_request_id
                        || existing.provider_request_id != command.candidate.provider_request_id
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if matches!(
                        existing.state,
                        GrantActivationState::Committed | GrantActivationState::Denied
                    ) || lease_expiry.is_some_and(|expires| expires > now)
                    {
                        return Ok(GrantActivationClaim {
                            acquired: false,
                            intent: existing,
                        });
                    }
                    if !existing.capacity_held {
                        store
                            .require_current_terms_in_transaction(
                                session,
                                &existing,
                                &command.acceptance,
                            )
                            .await?;
                        store
                            .reserve_activation_allocation_in_transaction(session, &existing)
                            .await?;
                        existing.capacity_held = true;
                    }
                    let expected_revision = existing.revision;
                    let (lease, expires_at_database) =
                        activation_lease(command.holder.clone(), now).map_err(custom)?;
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
                    .require_current_terms_in_transaction(session, &candidate, &command.acceptance)
                    .await?;
                store
                    .reserve_activation_allocation_in_transaction(session, &candidate)
                    .await?;
                let (lease, expires_at_database) =
                    activation_lease(command.holder.clone(), now).map_err(custom)?;
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
            .map(|document| read_activation(&self.cipher, document).map(|(intent, _)| intent))
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
        if command.operation.kind != ProviderOperationKind::RevokeUncommittedGrant
            || command.operation.phase != ProviderOperationPhase::Prepared
            || !is_digest(&command.holder)
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
                            && intent.lease.as_ref().map(|lease| lease.holder.as_str())
                                != Some(command.holder.as_str())
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
                        if existing.provider_request_id != command.operation.provider_request_id
                            || existing.payload_digest != command.operation.payload_digest
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
                let (lease, expires_at_database) =
                    activation_lease(command.holder.clone(), now).map_err(custom)?;
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
        if !is_digest(&command.holder) {
            return Err(SponsorshipStoreError::Malformed(
                "activation lease holder is not a digest".into(),
            ));
        }
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
                let (lease, expires_at_database) =
                    activation_lease(command.holder.clone(), now).map_err(custom)?;
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
            .launch_authority
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        command
            .acceptance
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
                let (mut intent, lease_expiry) = store
                    .activation_in_transaction(session, &command.launch_id)
                    .await?
                    .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                if intent.revision != command.expected_revision
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
                intent.updated_at = database_time_string(now);
                match store
                    .require_activation_dependencies_in_transaction(
                        session,
                        &intent,
                        &command.launch_authority,
                        &command.acceptance,
                        &intent.updated_at,
                    )
                    .await
                {
                    Ok(()) => {
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
        if !is_digest(holder) {
            return Err(SponsorshipStoreError::Malformed(
                "activation lease holder is not a digest".into(),
            ));
        }
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
                        || stored.launch_id != candidate.launch_id
                        || stored.session_id != candidate.session_id
                        || stored.policy_id != candidate.policy_id
                        || stored.policy_generation != candidate.policy_generation
                        || stored.credential_generation != candidate.credential_generation
                        || stored.launch_authority_digest != candidate.launch_authority_digest
                        || stored.acceptance_digest != candidate.acceptance_digest
                        || stored.gate_fence != candidate.gate_fence
                        || stored.allocation != candidate.allocation
                        || stored.request_fingerprint != candidate.request_fingerprint
                        || stored.billing_request_id != candidate.billing_request_id
                        || stored.provider_request_id != candidate.provider_request_id
                        || stored.created_at != candidate.created_at
                        || (candidate.lease.is_some() && candidate.lease != stored.lease)
                        || (stored.provider_receipt.is_some()
                            && candidate.provider_receipt.is_none())
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
        self.transaction((self.clone(), command), |session, (store, command)| {
            async move {
                let now = store.database_now(session).await?;
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
                {
                    if let Some(document) = store
                        .grants
                        .find_one(doc! { "launch_id": &command.grant.launch_id.0 })
                        .session(&mut *session)
                        .await?
                    {
                        let (grant, _) = read_grant(&store.cipher, document).map_err(custom)?;
                        return if grant == command.grant {
                            Ok(grant)
                        } else {
                            Err(custom(SponsorshipStoreError::Conflict))
                        };
                    }
                    return Err(custom(SponsorshipStoreError::StaleGeneration));
                }
                store
                    .require_activation_dependencies_in_transaction(
                        session,
                        &stored,
                        &command.launch_authority,
                        &command.acceptance,
                        &database_time_string(now),
                    )
                    .await?;
                store
                    .require_gate_in_transaction(session, &command.gate_fence)
                    .await?;
                if command.intent.revision
                    != command.expected_revision.checked_add(1).ok_or_else(|| {
                        custom(SponsorshipStoreError::Malformed(
                            "activation revision exhausted".into(),
                        ))
                    })?
                    || command.intent.launch_authority_digest != command.launch_authority.digest
                    || command.intent.acceptance_digest != command.acceptance.digest
                    || command.grant.acceptance != command.acceptance
                    || command.grant.allocation != stored.allocation
                    || command.grant.policy_id != stored.policy_id
                    || command.grant.policy_generation != stored.policy_generation
                    || command.grant.credential_generation != stored.credential_generation
                    || command.grant.launch_id != command.launch_authority.launch_id
                    || command.grant.contributor != command.launch_authority.contributor
                    || command.grant.trigger_revision_id != command.launch_authority.revision_id
                    || command.grant.trigger_revision_digest
                        != command.launch_authority.revision_digest
                    || command.launch_authority.session_id.as_ref()
                        != Some(&command.grant.session_id)
                {
                    return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                }
                let receipt = stored.provider_receipt.clone().ok_or_else(|| {
                    custom(SponsorshipStoreError::Malformed(
                        "confirmed activation has no provider receipt".into(),
                    ))
                })?;
                let database_now = database_time_seconds(now).map_err(custom)?;
                let issued = timestamp_seconds(&receipt.issued_at, "provider receipt issue time")
                    .map_err(custom)?;
                let expires = timestamp_seconds(&receipt.expires_at, "provider receipt expiry")
                    .map_err(custom)?;
                if issued > database_now || expires <= database_now {
                    return Err(custom(SponsorshipStoreError::GrantUnavailable));
                }
                require_receipt_matches_grant(&receipt, &command.grant).map_err(custom)?;
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
                    .insert_one(grant_doc(&store.cipher, &command.grant, &receipt).map_err(custom)?)
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
        .await
    }

    async fn complete_grant_activation_cleanup(
        &self,
        command: CompleteGrantActivationCleanup,
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
                    || stored.state != GrantActivationState::CleanupPending
                    || !stored.capacity_held
                    || stored.provider_receipt != command.intent.provider_receipt
                    || command.intent.state != GrantActivationState::Cleaned
                    || command.intent.capacity_held
                    || command.intent.lease.is_some()
                    || command.intent.cleanup_operation.is_some()
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
                    || command.intent.state != GrantActivationState::Denied
                    || command.intent.denial != Some(GrantActivationDenial::AuthorityInvalidated)
                    || command.intent.capacity_held
                    || command.intent.lease.is_some()
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
            .map(|document| read_grant(&self.cipher, document).map(|(_, receipt)| receipt))
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
            .map(|document| read_grant(&self.cipher, document).map(|(grant, _)| grant))
            .transpose()
    }

    async fn grant_for_launch(
        &self,
        launch_id: &LaunchId,
    ) -> Result<Option<SponsoredGrant>, SponsorshipStoreError> {
        self.grants
            .find_one(doc! { "launch_id": &launch_id.0 })
            .await
            .map_err(|error| stored_read_failure("read sponsored grant for Launch", error))?
            .map(|document| read_grant(&self.cipher, document).map(|(grant, _)| grant))
            .transpose()
    }

    async fn usage_reservation(
        &self,
        stable_request_id: &str,
    ) -> Result<Option<SponsoredUsageReservation>, SponsorshipStoreError> {
        self.reservations
            .find_one(doc! { "_id": stable_request_id })
            .await
            .map_err(|error| stored_read_failure("read sponsored usage reservation", error))?
            .map(|document| read_reservation(document).map(|(reservation, _, _)| reservation))
            .transpose()
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
        self.transaction(
            (self.clone(), expected_generation, grant.clone(), audit),
            |session, (store, expected, candidate, audit)| {
                async move {
                    let (stored, receipt) = store
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
                    if stored.usage != candidate.usage
                        || stored.allocation != candidate.allocation
                        || stored.id != candidate.id
                        || stored.policy_id != candidate.policy_id
                        || stored.launch_id != candidate.launch_id
                        || stored.credential_generation != candidate.credential_generation
                    {
                        return Err(custom(SponsorshipStoreError::StaleGeneration));
                    }
                    require_receipt_matches_grant(&receipt, candidate).map_err(custom)?;
                    store
                        .replace_grant_in_transaction(session, *expected, candidate, &receipt)
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
                    let (stored, receipt) = store
                        .grant_in_transaction(session, &candidate.id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
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
                        || candidate.generation
                            != expected.checked_add(1).ok_or_else(|| {
                                custom(SponsorshipStoreError::Malformed(
                                    "grant generation exhausted".into(),
                                ))
                            })?
                        || stored.policy_id != candidate.policy_id
                        || stored.usage != candidate.usage
                        || stored.allocation != candidate.allocation
                    {
                        return Err(custom(SponsorshipStoreError::Conflict));
                    }
                    if *reason == GrantCapacityRelease::Expired {
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
                    require_receipt_matches_grant(&receipt, candidate).map_err(custom)?;
                    store
                        .release_terminal_grant_capacity_in_transaction(session, &stored, None)
                        .await?;
                    store
                        .replace_grant_in_transaction(session, *expected, candidate, &receipt)
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
        command
            .launch_authority
            .validate()
            .map_err(|error| SponsorshipStoreError::Malformed(error.to_string()))?;
        validate_gate(&command.gate_fence)?;
        validate_audit_event(&audit)?;
        self.transaction(
            (self.clone(), command, request_fingerprint, audit),
            |session, (store, command, request_fingerprint, audit)| {
                async move {
                    if let Some((existing, _, _)) = store
                        .reservation_in_transaction(session, &command.request.stable_request_id)
                        .await?
                    {
                        if existing.idempotency_fingerprint
                            != command.request.idempotency_fingerprint
                        {
                            return Err(custom(SponsorshipStoreError::Conflict));
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
                    let current_launch = store
                        .launch_authority_in_transaction(
                            session,
                            &command.launch_authority.launch_id,
                        )
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::LaunchAuthorityChanged))?;
                    if current_launch != command.launch_authority
                        || !matches!(
                            current_launch.state,
                            SponsoredLaunchState::Active | SponsoredLaunchState::Recoverable
                        )
                    {
                        return Err(custom(SponsorshipStoreError::LaunchAuthorityChanged));
                    }
                    let (grant, _) = store
                        .grant_in_transaction(session, &command.request.grant_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                    let (policy, _) = store
                        .policy_in_transaction(session, &grant.policy_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                    let credential = store
                        .credential_record_in_transaction(session, &policy.credential_id)
                        .await?
                        .ok_or_else(|| custom(SponsorshipStoreError::GrantUnavailable))?;
                    if !grant.is_active_at(&now_string)
                        || !policy.is_available_at(&now_string)
                        || credential.state != SponsoredCredentialState::Active
                        || credential.material_state != SponsoredCredentialMaterialState::Retained
                        || credential.owner != policy.owner
                        || credential.authority.provider != policy.provider_policy.provider
                        || credential.authority.provider_policy_key
                            != policy.provider_policy.provider_policy_key
                        || grant.policy_generation != command.policy_generation
                        || policy.generation != command.policy_generation
                        || grant.credential_generation != command.credential_generation
                        || grant.contributor != command.request.requester
                        || grant.capability.fingerprint != command.request.capability_fingerprint
                        || grant.tenant_id != command.request.tenant_id
                        || grant.trigger_revision_id != command.request.trigger_revision_id
                        || grant.trigger_revision_digest != command.request.trigger_revision_digest
                        || grant.launch_id != command.request.launch_id
                        || grant.session_id != command.request.session_id
                        || grant.launch_id != current_launch.launch_id
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
                    let launch_held = store
                        .unresolved_usage_for_grant_in_transaction(session, &grant, None)
                        .await?;
                    let launch_projected = summary_add(&grant.usage, &launch_held)
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
                        stable_request_id: command.request.stable_request_id.clone(),
                        idempotency_fingerprint: command.request.idempotency_fingerprint.clone(),
                        request_fingerprint: request_fingerprint.clone(),
                        grant_id: grant.id,
                        price_quote: command.price_quote.clone(),
                        reserved: command.request.maximum.clone(),
                        state: SponsoredUsageState::Reserved,
                        created_at: command.now.clone(),
                        updated_at: command.now.clone(),
                        actual: None,
                    };
                    reservation.validate().map_err(|error| {
                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                    })?;
                    store
                        .reservations
                        .insert_one(reservation_doc(&reservation, &policy.id, now).map_err(custom)?)
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
        .await
    }

    async fn reconcile_usage(
        &self,
        command: ReconcileSponsoredUsage,
        audit: SponsoredAuditEvent,
    ) -> Result<SponsoredUsageReservation, SponsorshipStoreError> {
        validate_audit_event(&audit)?;
        self.transaction(
            (self.clone(), command, audit),
            |session, (store, command, audit)| {
                async move {
                    let (current, policy_id, rate_observed_at) = store
                        .reservation_in_transaction(session, &command.stable_request_id)
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
                        return Ok(current);
                    }
                    if matches!(
                        current.state,
                        SponsoredUsageState::Settled | SponsoredUsageState::Released
                    ) || !matches!(
                        command.outcome,
                        SponsoredUsageState::Settled
                            | SponsoredUsageState::Released
                            | SponsoredUsageState::Ambiguous
                    ) {
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
                    replacement.actual = command.actual.clone();
                    replacement.updated_at = command.observed_at.clone();
                    replacement.validate().map_err(|error| {
                        custom(SponsorshipStoreError::Malformed(error.to_string()))
                    })?;

                    if let Some(actual) = command
                        .actual
                        .as_ref()
                        .filter(|_| command.outcome == SponsoredUsageState::Settled)
                    {
                        let (mut grant, receipt) = store
                            .grant_in_transaction(session, &current.grant_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                        let expected_generation = grant.generation;
                        grant.usage = summary_add(&grant.usage, actual).map_err(custom)?;
                        grant.updated_at = replacement.updated_at.clone();
                        let capacity_was_released = grant_capacity_is_released(grant.state);
                        let becomes_exhausted = !capacity_was_released
                            && matches!(
                                grant.state,
                                SponsoredGrantState::Active | SponsoredGrantState::Suspended
                            )
                            && grant.usage.reaches_any_limit(&grant.allocation);
                        let (policy, mut usage) = store
                            .policy_in_transaction(session, &grant.policy_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                        usage.consumed = summary_add(&usage.consumed, actual).map_err(custom)?;
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
                                    Some(&current.stable_request_id),
                                )
                                .await?;
                        }
                        grant.validate().map_err(|error| {
                            custom(SponsorshipStoreError::Malformed(error.to_string()))
                        })?;
                        require_receipt_matches_grant(&receipt, &grant).map_err(custom)?;
                        store
                            .replace_grant_in_transaction(
                                session,
                                expected_generation,
                                &grant,
                                &receipt,
                            )
                            .await?;
                    } else if command.outcome == SponsoredUsageState::Released {
                        let (grant, _) = store
                            .grant_in_transaction(session, &current.grant_id)
                            .await?
                            .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                        if grant_capacity_is_released(grant.state) {
                            let (policy, mut usage) = store
                                .policy_in_transaction(session, &grant.policy_id)
                                .await?
                                .ok_or_else(|| custom(SponsorshipStoreError::Conflict))?;
                            usage.reserved =
                                summary_sub(&usage.reserved, &current.reserved).map_err(custom)?;
                            store
                                .replace_policy_usage_in_transaction(session, &policy, &usage)
                                .await?;
                        }
                    }

                    let result = store
                        .reservations
                        .replace_one(
                            doc! {
                                "_id": &current.stable_request_id,
                                "state": usage_state_code(current.state),
                            },
                            reservation_doc(&replacement, &policy_id, rate_observed_at)
                                .map_err(custom)?,
                        )
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

fn gate_doc(fence: &SponsorshipGateFence) -> Result<GateDoc, SponsorshipStoreError> {
    validate_gate(fence)?;
    Ok(GateDoc {
        id: GATE_ID.into(),
        document_schema_version: DOCUMENT_SCHEMA_VERSION,
        configuration_digest: fence.configuration_digest.clone(),
        epoch: signed(fence.epoch, "sponsorship gate epoch")?,
        kill_switch_engaged: fence.kill_switch_engaged,
    })
}

fn read_gate(document: GateDoc) -> Result<SponsorshipGateFence, SponsorshipStoreError> {
    validate_document_version(document.document_schema_version, "sponsorship gate")?;
    if document.id != GATE_ID {
        return Err(SponsorshipStoreError::Malformed(
            "stored sponsorship gate has an unsupported identity".into(),
        ));
    }
    let fence = SponsorshipGateFence {
        configuration_digest: document.configuration_digest,
        epoch: unsigned(document.epoch, "sponsorship gate epoch")?,
        kill_switch_engaged: document.kill_switch_engaged,
    };
    validate_gate(&fence)?;
    Ok(fence)
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
    use super::super::sponsorship_encryption::SponsorshipEncryptionKey;
    use super::*;

    const TEST_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    const TEST_NOW: &str = "2026-01-02T03:04:05Z";

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
                provider_credential_id_digest: "a".repeat(64),
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
        let fence = SponsorshipGateFence {
            configuration_digest: "a".repeat(64),
            epoch: 1,
            kill_switch_engaged: true,
        };
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
    }

    #[test]
    fn sponsorship_metadata_read_never_authenticates_credential_secrets() {
        let cipher = test_cipher();
        let record = credential_fixture();
        let document =
            credential_doc(&cipher, &record, Some("provider-credential-secret"), None).unwrap();

        let mut raw = mongodb::bson::serialize_to_document(&document).unwrap();
        let current_secret = raw
            .get_document_mut("current_secret")
            .expect("fixture retains current material");
        match current_secret.get_mut("ciphertext") {
            Some(Bson::Binary(ciphertext)) => ciphertext.bytes[0] ^= 0x80,
            _ => panic!("credential ciphertext is stored as BSON binary"),
        }
        let document = mongodb::bson::deserialize_from_document(raw).unwrap();

        assert_eq!(read_credential_record(&cipher, &document).unwrap(), record);
        let error = match read_credential(&cipher, document) {
            Err(error) => error,
            Ok(_) => panic!("tampered credential material must fail authentication"),
        };
        assert!(matches!(error, SponsorshipStoreError::Malformed(_)));
        assert!(!error.to_string().contains("provider-credential-secret"));
    }

    #[test]
    fn sponsorship_credential_bson_rejects_index_and_schema_drift() {
        let cipher = test_cipher();
        let record = credential_fixture();
        let document =
            credential_doc(&cipher, &record, Some("provider-credential-secret"), None).unwrap();
        assert_eq!(
            read_credential(&cipher, document.clone()).unwrap().record,
            record
        );

        let mut unsupported = document.clone();
        unsupported.document_schema_version += 1;
        assert!(matches!(
            read_credential_record(&cipher, &unsupported),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut wrong_owner_index = document.clone();
        wrong_owner_index.owner_subject = "different-owner".into();
        assert!(matches!(
            read_credential_record(&cipher, &wrong_owner_index),
            Err(SponsorshipStoreError::Malformed(_))
        ));

        let mut wrong_key_index = document;
        wrong_key_index.encryption_key_id = "unknown-key".into();
        assert!(matches!(
            read_credential_record(&cipher, &wrong_key_index),
            Err(SponsorshipStoreError::Malformed(_))
        ));
    }

    #[test]
    fn sponsorship_activation_leases_are_derived_from_database_time() {
        let now = DateTime::from_millis(1_700_000_000_123);
        let (lease, expiry) = activation_lease("b".repeat(64), now).unwrap();
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

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set or mongos"]
    async fn real_mongo_sponsorship_startup_commits_transaction_probe_and_builds_indexes() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("ignored test requires CHRONO_TEST_MONGODB_URI");
        let client = Client::with_uri_str(&uri)
            .await
            .expect("test Mongo connects");
        let database_name = format!(
            "chrono_sponsorship_startup_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        );
        let gate = SponsorshipGateFence {
            configuration_digest: "c".repeat(64),
            epoch: 1,
            kill_switch_engaged: false,
        };
        let store = MongoSponsorshipStore::open(
            &client,
            &database_name,
            "test-deployment",
            test_keyring(),
            gate.clone(),
        )
        .await
        .expect("replica-set sponsorship store opens");
        assert_eq!(store.gate_fence().await.unwrap(), gate);

        let credential_indexes = store.credentials.list_index_names().await.unwrap();
        assert!(credential_indexes
            .iter()
            .any(|name| name == "sponsorship_credential_provider_identity"));
        assert!(credential_indexes
            .iter()
            .any(|name| name == "sponsorship_credentials_by_owner"));
        let reservation_indexes = store.reservations.list_index_names().await.unwrap();
        assert!(reservation_indexes
            .iter()
            .any(|name| name == "sponsorship_usage_rate_window"));
        let activation_indexes = store.activations.list_index_names().await.unwrap();
        assert!(activation_indexes
            .iter()
            .any(|name| name == "sponsorship_activations_due"));

        client
            .database(&database_name)
            .drop()
            .await
            .expect("temporary sponsorship database drops");
    }
}
