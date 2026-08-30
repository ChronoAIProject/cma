//! Sponsored credential, policy, grant, usage, and audit domain models.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use ring::hmac;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use zeroize::Zeroizing;

use super::super::{
    BillingMode, CredentialRoute, Launch, LaunchId, LaunchState, TriggerRevisionId,
    TriggerValidationError, CMA_TRIGGER_SPONSORED_POLICY_V1,
};
use super::lifecycle::SponsorshipError;
use super::ports::ProviderGrantReceipt;
use super::validation::{
    canonical_digest, lower_hex, validate_digest, validate_display_name, validate_identifier,
    validate_policy_token, validate_timestamp,
};
use crate::{SessionId, SubjectId};

pub const SPONSORSHIP_SCHEMA_VERSION: u32 = 1;
pub const MAX_SPONSORED_MODELS: usize = 32;
pub const MAX_SPONSORED_RESOURCES: usize = 32;
pub const MAX_SPONSORED_AUDIT_PAGE: usize = 200;
pub const MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS: usize = 4;
/// Maximum canonical Responses JSON retained for one recoverable dispatch.
pub const MAX_SPONSORED_MODEL_REQUEST_BYTES: usize = 1024 * 1024;
pub const SPONSORED_MODEL_DISPATCH_PAYLOAD_SCHEMA_VERSION: u32 = 1;
/// Maximum canonical provider response retained for exact settled replay.
pub const MAX_SPONSORED_MODEL_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
pub const SPONSORED_MODEL_RESPONSE_PAYLOAD_SCHEMA_VERSION: u32 = 1;

const SPONSORSHIP_OPERATIONAL_PSEUDONYM_PREFIX: &str = "sp1.";
const SPONSORSHIP_OPERATIONAL_KEY_BYTES: usize = 32;
const MAX_SPONSORSHIP_OPERATIONAL_CONTEXT_COMPONENTS: usize = 16;
const MAX_SPONSORSHIP_OPERATIONAL_CONTEXT_BYTES: usize = 256 * 1024;

fn validate_operational_key_id(key_id: &str) -> Result<(), TriggerValidationError> {
    if key_id.is_empty()
        || key_id.len() > 64
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(TriggerValidationError::field(
            "sponsorship_operational_key_id",
            "must contain 1 to 64 safe ASCII bytes",
        ));
    }
    Ok(())
}

/// A deployment-keyed durable lookup value.
///
/// The key id is part of the canonical representation so a bounded current and
/// predecessor keyring can recognize retries during rotation. It is parsed at
/// the final separator because valid encryption key ids may themselves contain
/// `.`. Configuration-relative validation is performed by
/// [`SponsorshipOperationalPseudonymizer::require_configured`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct SponsorshipOperationalPseudonym(String);

impl SponsorshipOperationalPseudonym {
    pub fn parse(value: impl Into<String>) -> Result<Self, TriggerValidationError> {
        let value = value.into();
        let encoded = value
            .strip_prefix(SPONSORSHIP_OPERATIONAL_PSEUDONYM_PREFIX)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "sponsorship_operational_pseudonym",
                    "must use the canonical sp1 encoding",
                )
            })?;
        let (key_id, digest) = encoded.rsplit_once('.').ok_or_else(|| {
            TriggerValidationError::field(
                "sponsorship_operational_pseudonym",
                "must contain a key id and digest",
            )
        })?;
        validate_operational_key_id(key_id)?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_pseudonym",
                "must contain a 64-character lowercase hexadecimal digest",
            ));
        }
        let canonical = format!("{SPONSORSHIP_OPERATIONAL_PSEUDONYM_PREFIX}{key_id}.{digest}");
        if canonical != value {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_pseudonym",
                "must use one canonical spelling",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn key_id(&self) -> &str {
        self.0
            .strip_prefix(SPONSORSHIP_OPERATIONAL_PSEUDONYM_PREFIX)
            .and_then(|value| value.rsplit_once('.'))
            .map(|(key_id, _)| key_id)
            .expect("construction validates the canonical pseudonym encoding")
    }
}

impl<'de> Deserialize<'de> for SponsorshipOperationalPseudonym {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for SponsorshipOperationalPseudonym {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The current operational pseudonym followed by the bounded predecessor-key
/// candidates that may identify an existing durable binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipOperationalPseudonymSet {
    current: SponsorshipOperationalPseudonym,
    previous: Vec<SponsorshipOperationalPseudonym>,
}

impl SponsorshipOperationalPseudonymSet {
    fn new(
        current: SponsorshipOperationalPseudonym,
        previous: Vec<SponsorshipOperationalPseudonym>,
    ) -> Result<Self, TriggerValidationError> {
        if previous.len() > MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_pseudonyms",
                "contains too many predecessor-key candidates",
            ));
        }
        let mut unique = BTreeSet::from([current.clone()]);
        if previous
            .iter()
            .any(|candidate| !unique.insert(candidate.clone()))
        {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_pseudonyms",
                "must not contain duplicate candidates",
            ));
        }
        Ok(Self { current, previous })
    }

    pub fn current(&self) -> &SponsorshipOperationalPseudonym {
        &self.current
    }

    pub fn iter(&self) -> impl Iterator<Item = &SponsorshipOperationalPseudonym> {
        std::iter::once(&self.current).chain(self.previous.iter())
    }

    pub fn matches(&self, candidate: &SponsorshipOperationalPseudonym) -> bool {
        self.iter().any(|value| value == candidate)
    }

    pub fn encoded_candidates(&self) -> Vec<String> {
        self.iter()
            .map(|value| value.as_str().to_string())
            .collect()
    }
}

/// Fixed domain separation for every caller-controlled or provider-sensitive
/// durable identity. Callers cannot invent ad hoc domain strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SponsorshipOperationalDomain {
    CredentialRegistrationRequest,
    CredentialRegistrationCommand,
    PolicyCreationRequest,
    PolicyCreationCommand,
    CredentialReplacement,
    CredentialOperationRequest,
    AcceptanceRequest,
    ActivationRequest,
    UsageRequest,
    GrantOperationRequest,
    ProviderCredentialAuthority,
    ProviderGrantAuthority,
    ProviderRevocationAuthority,
}

impl SponsorshipOperationalDomain {
    fn label(self) -> &'static str {
        match self {
            Self::CredentialRegistrationRequest => {
                "cma-sponsored-operational-registration-request-v1"
            }
            Self::CredentialRegistrationCommand => {
                "cma-sponsored-operational-registration-command-v1"
            }
            Self::PolicyCreationRequest => "cma-sponsored-operational-policy-creation-request-v1",
            Self::PolicyCreationCommand => "cma-sponsored-operational-policy-creation-command-v1",
            Self::CredentialReplacement => "cma-sponsored-operational-credential-replacement-v1",
            Self::CredentialOperationRequest => {
                "cma-sponsored-operational-credential-operation-request-v1"
            }
            Self::AcceptanceRequest => "cma-sponsored-operational-acceptance-request-v1",
            Self::ActivationRequest => "cma-sponsored-operational-activation-request-v1",
            Self::UsageRequest => "cma-sponsored-operational-usage-request-v1",
            Self::GrantOperationRequest => "cma-sponsored-operational-grant-operation-request-v1",
            Self::ProviderCredentialAuthority => {
                "cma-sponsored-operational-provider-credential-authority-v1"
            }
            Self::ProviderGrantAuthority => "cma-sponsored-operational-provider-grant-authority-v1",
            Self::ProviderRevocationAuthority => {
                "cma-sponsored-operational-provider-revocation-authority-v1"
            }
        }
    }
}

/// Bounded operational HMAC keyring built from its own long-lived root-key
/// class. Encryption keys must never enter this type.
pub struct SponsorshipOperationalPseudonymizer {
    current_key_id: String,
    keys: BTreeMap<String, Zeroizing<Vec<u8>>>,
}

impl SponsorshipOperationalPseudonymizer {
    pub fn new(
        current_key_id: impl Into<String>,
        current_key: impl Into<Vec<u8>>,
        previous: impl IntoIterator<Item = (String, Vec<u8>)>,
    ) -> Result<Self, TriggerValidationError> {
        let current_key_id = current_key_id.into();
        let current_key = Zeroizing::new(current_key.into());
        let previous = previous
            .into_iter()
            .map(|(key_id, key)| (key_id, Zeroizing::new(key)))
            .collect::<Vec<_>>();
        validate_operational_key_id(&current_key_id)?;
        if current_key.len() != SPONSORSHIP_OPERATIONAL_KEY_BYTES {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_key",
                "must contain exactly 32 derived bytes",
            ));
        }
        if previous.len() > MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_keyring",
                "contains too many predecessor keys",
            ));
        }
        let mut keys = BTreeMap::from([(current_key_id.clone(), current_key)]);
        for (key_id, key) in previous {
            validate_operational_key_id(&key_id)?;
            if key.len() != SPONSORSHIP_OPERATIONAL_KEY_BYTES {
                return Err(TriggerValidationError::field(
                    "sponsorship_operational_key",
                    "must contain exactly 32 derived bytes",
                ));
            }
            if keys.contains_key(&key_id) {
                return Err(TriggerValidationError::field(
                    "sponsorship_operational_keyring",
                    "contains a duplicate key id",
                ));
            }
            if keys
                .values()
                .any(|configured| configured.as_slice() == key.as_slice())
            {
                return Err(TriggerValidationError::field(
                    "sponsorship_operational_keyring",
                    "contains duplicate key material under different ids",
                ));
            }
            keys.insert(key_id, key);
        }
        Ok(Self {
            current_key_id,
            keys,
        })
    }

    pub fn current_key_id(&self) -> &str {
        &self.current_key_id
    }

    pub fn configured_key_ids(&self) -> impl Iterator<Item = &str> {
        self.keys.keys().map(String::as_str)
    }

    pub fn require_configured(
        &self,
        candidate: &SponsorshipOperationalPseudonym,
    ) -> Result<(), TriggerValidationError> {
        if !self.keys.contains_key(candidate.key_id()) {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_pseudonym",
                "references an unconfigured key id",
            ));
        }
        Ok(())
    }

    pub fn parse_configured(
        &self,
        value: impl Into<String>,
    ) -> Result<SponsorshipOperationalPseudonym, TriggerValidationError> {
        let candidate = SponsorshipOperationalPseudonym::parse(value)?;
        self.require_configured(&candidate)?;
        Ok(candidate)
    }

    pub fn pseudonymize(
        &self,
        domain: SponsorshipOperationalDomain,
        context: &[&str],
    ) -> Result<SponsorshipOperationalPseudonymSet, TriggerValidationError> {
        if context.is_empty()
            || context.len() > MAX_SPONSORSHIP_OPERATIONAL_CONTEXT_COMPONENTS
            || context.iter().any(|value| value.len() > 64 * 1024)
        {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_context",
                "must contain bounded context components",
            ));
        }
        let context_bytes = context
            .iter()
            .try_fold(domain.label().len() + 16, |length, value| {
                length.checked_add(value.len() + 8)
            });
        let context_bytes = context_bytes.ok_or_else(|| {
            TriggerValidationError::field("sponsorship_operational_context", "is too large")
        })?;
        if context_bytes > MAX_SPONSORSHIP_OPERATIONAL_CONTEXT_BYTES {
            return Err(TriggerValidationError::field(
                "sponsorship_operational_context",
                "is too large",
            ));
        }
        let mut message = Zeroizing::new(Vec::with_capacity(context_bytes));
        append_operational_part(&mut message, domain.label().as_bytes());
        for value in context {
            append_operational_part(&mut message, value.as_bytes());
        }

        let pseudonym_for =
            |key_id: &str,
             key: &Zeroizing<Vec<u8>>|
             -> Result<SponsorshipOperationalPseudonym, TriggerValidationError> {
                let key = hmac::Key::new(hmac::HMAC_SHA256, key.as_slice());
                let digest = lower_hex(hmac::sign(&key, message.as_slice()).as_ref());
                SponsorshipOperationalPseudonym::parse(format!(
                    "{SPONSORSHIP_OPERATIONAL_PSEUDONYM_PREFIX}{key_id}.{digest}"
                ))
            };
        let current = pseudonym_for(
            &self.current_key_id,
            self.keys
                .get(&self.current_key_id)
                .expect("construction inserts the current operational key"),
        )?;
        let previous = self
            .keys
            .iter()
            .filter(|(key_id, _)| *key_id != &self.current_key_id)
            .map(|(key_id, key)| pseudonym_for(key_id, key))
            .collect::<Result<Vec<_>, _>>()?;
        SponsorshipOperationalPseudonymSet::new(current, previous)
    }
}

impl fmt::Debug for SponsorshipOperationalPseudonymizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SponsorshipOperationalPseudonymizer")
            .field("current_key_id", &self.current_key_id)
            .field("key_count", &self.keys.len())
            .finish()
    }
}

fn append_operational_part(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as u64).to_be_bytes());
    target.extend_from_slice(value);
}

fn timestamp_not_after(first: &str, second: &str) -> bool {
    crate::seconds_from_rfc3339(first)
        .zip(crate::seconds_from_rfc3339(second))
        .is_some_and(|(first, second)| first <= second)
}

fn timestamp_before(first: &str, second: &str) -> bool {
    crate::seconds_from_rfc3339(first)
        .zip(crate::seconds_from_rfc3339(second))
        .is_some_and(|(first, second)| first < second)
}

fn operation_is_within_record(
    operation: &PendingProviderOperation,
    created_at: &str,
    updated_at: &str,
) -> bool {
    timestamp_not_after(created_at, &operation.requested_at)
        && timestamp_not_after(&operation.requested_at, updated_at)
        && operation
            .attempted_at
            .as_ref()
            .is_none_or(|attempted_at| timestamp_not_after(attempted_at, updated_at))
}

macro_rules! sponsorship_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4().to_string())
            }

            pub fn parse(value: impl Into<String>) -> Result<Self, TriggerValidationError> {
                let value = value.into();
                let parsed = Uuid::parse_str(&value).map_err(|_| {
                    TriggerValidationError::field(stringify!($name), "must be a canonical UUID")
                })?;
                if parsed.to_string() != value {
                    return Err(TriggerValidationError::field(
                        stringify!($name),
                        "must be a lowercase hyphenated canonical UUID",
                    ));
                }
                Ok(Self(value))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

sponsorship_id!(SponsoredCredentialId);
sponsorship_id!(SponsorshipPolicyId);
sponsorship_id!(SponsoredGrantId);
sponsorship_id!(SponsoredUsageReservationId);

/// Provider credential bytes accepted from an authenticated owner.
///
/// This type deliberately has no serialization implementation and its debug
/// form is always redacted. Dropping it zeroes the owned allocation.
pub struct ProviderCredentialSecret(Zeroizing<String>);

impl ProviderCredentialSecret {
    pub fn new(value: impl Into<String>) -> Result<Self, TriggerValidationError> {
        let value = Zeroizing::new(value.into());
        if value.is_empty() || value.len() > 64 * 1024 {
            return Err(TriggerValidationError::field(
                "credential",
                "must contain between 1 and 65536 bytes",
            ));
        }
        Ok(Self(value))
    }

    /// Exposed only so the selected encrypted store and provider adapter can
    /// use the value. It must never be logged, serialized, or sent to a runtime.
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }
}

/// Raw provider authority that may cross only the provider adapter and the
/// encrypted sponsorship persistence boundary.
///
/// It deliberately implements neither `Serialize` nor `Deserialize`, and its
/// diagnostics are always redacted. A durable store must encrypt the exposed
/// bytes before writing them; public projections contain only independently
/// domain-separated digests.
pub struct SensitiveProviderValue(Zeroizing<String>);

impl SensitiveProviderValue {
    pub fn new(value: impl Into<String>) -> Result<Self, TriggerValidationError> {
        let value = Zeroizing::new(value.into());
        validate_identifier("provider_authority", &value, 512)?;
        Ok(Self(value))
    }

    /// Available only to provider adapters and encrypted store
    /// implementations. The returned value must never enter diagnostics,
    /// public serialization, a prompt, a runtime, or a workspace.
    pub fn expose_for_provider_or_encryption(&self) -> &str {
        self.0.as_str()
    }
}

impl Clone for SensitiveProviderValue {
    fn clone(&self) -> Self {
        Self(Zeroizing::new(self.0.to_string()))
    }
}

impl PartialEq for SensitiveProviderValue {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

impl Eq for SensitiveProviderValue {}

impl fmt::Debug for SensitiveProviderValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveProviderValue(<redacted>)")
    }
}

impl fmt::Debug for ProviderCredentialSecret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderCredentialSecret(<redacted>)")
    }
}

/// Deployment-owned key used only to pseudonymize audit identifiers.
///
/// A stable key preserves audit correlation across restarts, while HMAC makes
/// low-entropy subject names and caller-selected request IDs resistant to an
/// offline dictionary attack against an exported audit collection. The key is
/// never serializable or exposed by this type and is zeroed on drop.
pub struct SponsorshipAuditPseudonymizer {
    key: Zeroizing<Vec<u8>>,
}

impl SponsorshipAuditPseudonymizer {
    pub fn new(key: impl Into<Vec<u8>>) -> Result<Self, TriggerValidationError> {
        let key = Zeroizing::new(key.into());
        if !(32..=1024).contains(&key.len()) {
            return Err(TriggerValidationError::field(
                "sponsorship_audit_key",
                "must contain between 32 and 1024 bytes",
            ));
        }
        Ok(Self { key })
    }

    fn pseudonymize(&self, domain: &str, value: &str) -> String {
        let mut message = Zeroizing::new(Vec::with_capacity(domain.len() + value.len() + 9));
        message.extend_from_slice(domain.as_bytes());
        message.push(0);
        message.extend_from_slice(&(value.len() as u64).to_be_bytes());
        message.extend_from_slice(value.as_bytes());
        let key = hmac::Key::new(hmac::HMAC_SHA256, self.key.as_slice());
        lower_hex(hmac::sign(&key, message.as_slice()).as_ref())
    }

    pub fn actor_digest(&self, actor: &SubjectId) -> String {
        self.pseudonymize("cma-sponsored-audit-actor-v1", &actor.0)
    }

    fn request_digest(&self, stable_request_id: &str) -> String {
        self.pseudonymize("cma-sponsored-audit-request-v1", stable_request_id)
    }
}

impl fmt::Debug for SponsorshipAuditPseudonymizer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SponsorshipAuditPseudonymizer(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SponsorBillingOwner {
    Developer {
        subject: SubjectId,
        display_name: String,
    },
    Organization {
        organization_id: String,
        administrator: SubjectId,
        display_name: String,
    },
}

impl SponsorBillingOwner {
    pub fn controller(&self) -> &SubjectId {
        match self {
            Self::Developer { subject, .. } => subject,
            Self::Organization { administrator, .. } => administrator,
        }
    }

    pub fn display_name(&self) -> &str {
        match self {
            Self::Developer { display_name, .. } | Self::Organization { display_name, .. } => {
                display_name
            }
        }
    }

    /// Stable deployment-local admission identity for one billing sponsor.
    ///
    /// Display names and organization administrators are mutable presentation
    /// or control data, so neither may partition sponsor-wide policy and Launch
    /// ceilings. Developer subjects and organization ids are the durable
    /// authority components instead.
    pub fn sponsorship_authority_key(&self) -> Result<String, TriggerValidationError> {
        self.validate()?;
        match self {
            Self::Developer { subject, .. } => canonical_digest(&(
                "cma-sponsorship-sponsor-authority-v1",
                "developer",
                &subject.0,
            )),
            Self::Organization {
                organization_id, ..
            } => canonical_digest(&(
                "cma-sponsorship-sponsor-authority-v1",
                "organization",
                organization_id,
            )),
        }
    }

    pub(super) fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_identifier("billing_owner.controller", &self.controller().0, 255)?;
        validate_display_name("billing_owner.display_name", self.display_name())?;
        if let Self::Organization {
            organization_id, ..
        } = self
        {
            validate_identifier("billing_owner.organization_id", organization_id, 255)?;
        }
        Ok(())
    }
}

/// Internal provider evidence retained after ownership validation.
///
/// `provider_policy_handle` is not credential material, but it is deliberately
/// absent from every public projection so callers cannot turn an implementation
/// locator into an accidental API contract.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderCredentialAuthority {
    pub provider: String,
    pub provider_policy_key: String,
    pub provider_policy_handle: SensitiveProviderValue,
    pub provider_credential_id_digest: String,
    pub state_version: u64,
    pub ownership_verified_at: String,
    pub supports_expiry: bool,
    pub supports_revocation: bool,
}

impl fmt::Debug for ProviderCredentialAuthority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderCredentialAuthority")
            .field("provider", &self.provider)
            .field("provider_policy_key", &self.provider_policy_key)
            .field("provider_policy_handle", &"<redacted>")
            .field(
                "provider_credential_id_digest",
                &self.provider_credential_id_digest,
            )
            .field("state_version", &self.state_version)
            .field("ownership_verified_at", &self.ownership_verified_at)
            .field("supports_expiry", &self.supports_expiry)
            .field("supports_revocation", &self.supports_revocation)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SponsoredCredentialState {
    Active,
    RotationPending,
    RevocationPending,
    Revoked,
}

/// Whether decrypted provider material must remain recoverable for a pending
/// provider mutation or has been retired after definitive global revocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SponsoredCredentialMaterialState {
    Retained,
    Retired,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsoredCredentialRecord {
    pub schema_version: u32,
    pub id: SponsoredCredentialId,
    pub owner: SponsorBillingOwner,
    pub authority: ProviderCredentialAuthority,
    pub state: SponsoredCredentialState,
    pub material_state: SponsoredCredentialMaterialState,
    pub generation: u64,
    pub created_at: String,
    pub updated_at: String,
    pub revoked_at: Option<String>,
    pub pending_operation: Option<PendingProviderOperation>,
    pub superseded_operation: Option<PendingProviderOperation>,
    pub last_completed_operation: Option<CompletedProviderOperation>,
}

impl fmt::Debug for SponsoredCredentialRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SponsoredCredentialRecord")
            .field("schema_version", &self.schema_version)
            .field("id", &self.id)
            .field("owner", &self.owner)
            .field("authority", &self.authority)
            .field("state", &self.state)
            .field("material_state", &self.material_state)
            .field("generation", &self.generation)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .field("revoked_at", &self.revoked_at)
            .field("pending_operation", &self.pending_operation)
            .field("superseded_operation", &self.superseded_operation)
            .field("last_completed_operation", &self.last_completed_operation)
            .finish()
    }
}

impl SponsoredCredentialRecord {
    /// Whether retained root authority may be opened to observe existing model
    /// custody. Rotation keeps the prior root material readable for recovery,
    /// but a provider-dispatch claim remains blocked until rotation resolves.
    /// Revocation recovery is allowed only before its first provider attempt.
    pub fn permits_retained_model_recovery_authority(&self) -> bool {
        self.material_state == SponsoredCredentialMaterialState::Retained
            && match self.state {
                SponsoredCredentialState::Active | SponsoredCredentialState::RotationPending => {
                    true
                }
                SponsoredCredentialState::RevocationPending => {
                    self.pending_operation.as_ref().is_some_and(|operation| {
                        operation.kind == ProviderOperationKind::RevokeCredential
                            && operation.phase == ProviderOperationPhase::Prepared
                            && operation.attempted_at.is_none()
                    })
                }
                SponsoredCredentialState::Revoked => false,
            }
    }

    /// Whether already-encrypted model request custody may cross another
    /// provider-dispatch claim. New reservations require `Active`; this
    /// narrower recovery exception exists only while revocation is durably
    /// fail-closed but has not begun at the provider.
    pub fn permits_retained_model_dispatch(&self) -> bool {
        self.material_state == SponsoredCredentialMaterialState::Retained
            && match self.state {
                SponsoredCredentialState::Active => true,
                SponsoredCredentialState::RevocationPending => {
                    self.pending_operation.as_ref().is_some_and(|operation| {
                        operation.kind == ProviderOperationKind::RevokeCredential
                            && operation.phase == ProviderOperationPhase::Prepared
                            && operation.attempted_at.is_none()
                    })
                }
                SponsoredCredentialState::RotationPending | SponsoredCredentialState::Revoked => {
                    false
                }
            }
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.schema_version != SPONSORSHIP_SCHEMA_VERSION || self.generation == 0 {
            return Err(TriggerValidationError::field(
                "credential",
                "has an unsupported schema or zero generation",
            ));
        }
        SponsoredCredentialId::parse(self.id.0.clone())?;
        self.owner.validate()?;
        validate_policy_token("credential.provider", &self.authority.provider)?;
        validate_policy_token(
            "credential.provider_policy_key",
            &self.authority.provider_policy_key,
        )?;
        validate_identifier(
            "credential.provider_policy_handle",
            self.authority
                .provider_policy_handle
                .expose_for_provider_or_encryption(),
            512,
        )?;
        SponsorshipOperationalPseudonym::parse(
            self.authority.provider_credential_id_digest.clone(),
        )?;
        validate_timestamp(
            "credential.ownership_verified_at",
            &self.authority.ownership_verified_at,
        )?;
        validate_timestamp("credential.created_at", &self.created_at)?;
        validate_timestamp("credential.updated_at", &self.updated_at)?;
        if !timestamp_not_after(&self.created_at, &self.updated_at) {
            return Err(TriggerValidationError::field(
                "credential.updated_at",
                "must not precede creation",
            ));
        }
        if self.authority.state_version == 0
            || !self.authority.supports_expiry
            || !self.authority.supports_revocation
        {
            return Err(TriggerValidationError::field(
                "credential.authority",
                "must prove nonzero provider state plus expiry and revocation support",
            ));
        }
        if (self.state == SponsoredCredentialState::Revoked) != self.revoked_at.is_some() {
            return Err(TriggerValidationError::field(
                "credential.revoked_at",
                "must exist exactly for a revoked credential",
            ));
        }
        if let Some(revoked_at) = &self.revoked_at {
            validate_timestamp("credential.revoked_at", revoked_at)?;
            if !timestamp_not_after(&self.created_at, revoked_at)
                || !timestamp_not_after(revoked_at, &self.updated_at)
            {
                return Err(TriggerValidationError::field(
                    "credential.revoked_at",
                    "must fall between creation and the latest update",
                ));
            }
        }
        if (self.state == SponsoredCredentialState::Revoked)
            != (self.material_state == SponsoredCredentialMaterialState::Retired)
        {
            return Err(TriggerValidationError::field(
                "credential.material_state",
                "must retire exactly after definitive credential revocation",
            ));
        }
        if matches!(
            self.state,
            SponsoredCredentialState::RotationPending | SponsoredCredentialState::RevocationPending
        ) != self.pending_operation.is_some()
        {
            return Err(TriggerValidationError::field(
                "credential.pending_operation",
                "must exist exactly while a provider mutation is pending",
            ));
        }
        if let Some(operation) = &self.pending_operation {
            operation.validate()?;
            if !operation_is_within_record(operation, &self.created_at, &self.updated_at) {
                return Err(TriggerValidationError::field(
                    "credential.pending_operation",
                    "must fall within the credential timestamp lineage",
                ));
            }
            let expected = match self.state {
                SponsoredCredentialState::RotationPending => {
                    ProviderOperationKind::RotateCredential
                }
                SponsoredCredentialState::RevocationPending => {
                    ProviderOperationKind::RevokeCredential
                }
                SponsoredCredentialState::Active | SponsoredCredentialState::Revoked => {
                    return Err(TriggerValidationError::field(
                        "credential.pending_operation",
                        "cannot accompany an active or revoked credential",
                    ))
                }
            };
            if operation.kind != expected {
                return Err(TriggerValidationError::field(
                    "credential.pending_operation.kind",
                    "does not match the credential lifecycle state",
                ));
            }
        }
        if let Some(operation) = &self.superseded_operation {
            operation.validate()?;
            if !operation_is_within_record(operation, &self.created_at, &self.updated_at) {
                return Err(TriggerValidationError::field(
                    "credential.superseded_operation",
                    "must fall within the credential timestamp lineage",
                ));
            }
            if self.state != SponsoredCredentialState::RevocationPending
                || operation.kind != ProviderOperationKind::RotateCredential
            {
                return Err(TriggerValidationError::field(
                    "credential.superseded_operation",
                    "may only retain a rotation superseded by emergency revocation",
                ));
            }
        }
        if let Some(operation) = &self.last_completed_operation {
            operation.validate()?;
            if !timestamp_not_after(&self.created_at, &operation.completed_at)
                || !timestamp_not_after(&operation.completed_at, &self.updated_at)
            {
                return Err(TriggerValidationError::field(
                    "credential.last_completed_operation.completed_at",
                    "must fall between credential creation and update",
                ));
            }
            if !matches!(
                operation.kind,
                ProviderOperationKind::RotateCredential | ProviderOperationKind::RevokeCredential
            ) {
                return Err(TriggerValidationError::field(
                    "credential.last_completed_operation.kind",
                    "must be a credential operation",
                ));
            }
            if operation.kind == ProviderOperationKind::RevokeCredential
                && operation.outcome == CompletedProviderOperationOutcome::Confirmed
                && self.state != SponsoredCredentialState::Revoked
            {
                return Err(TriggerValidationError::field(
                    "credential.last_completed_operation",
                    "confirmed revocation must leave the credential revoked",
                ));
            }
        }
        if self.material_state == SponsoredCredentialMaterialState::Retired
            && !matches!(
                self.last_completed_operation.as_ref(),
                Some(CompletedProviderOperation {
                    kind: ProviderOperationKind::RevokeCredential,
                    outcome: CompletedProviderOperationOutcome::Confirmed,
                    ..
                })
            )
        {
            return Err(TriggerValidationError::field(
                "credential.last_completed_operation",
                "must prove confirmed provider revocation before material retirement",
            ));
        }
        if self.material_state == SponsoredCredentialMaterialState::Retired
            && self.revoked_at.as_ref()
                != self
                    .last_completed_operation
                    .as_ref()
                    .map(|operation| &operation.completed_at)
        {
            return Err(TriggerValidationError::field(
                "credential.revoked_at",
                "must equal the confirmed provider revocation completion time",
            ));
        }
        Ok(())
    }
}

/// Owner-facing credential metadata. No provider handle, external identifier,
/// ciphertext, or secret value crosses this projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredCredentialView {
    pub id: SponsoredCredentialId,
    pub provider: String,
    pub provider_policy_key: String,
    pub state: SponsoredCredentialState,
    pub generation: u64,
    pub ownership_verified_at: String,
    pub updated_at: String,
}

impl From<&SponsoredCredentialRecord> for SponsoredCredentialView {
    fn from(value: &SponsoredCredentialRecord) -> Self {
        Self {
            id: value.id.clone(),
            provider: value.authority.provider.clone(),
            provider_policy_key: value.authority.provider_policy_key.clone(),
            state: value.state,
            generation: value.generation,
            ownership_verified_at: value.authority.ownership_verified_at.clone(),
            updated_at: value.updated_at.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SponsoredQuota {
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// Deployment-owned ceilings applied to every sponsored policy and Launch.
///
/// This value is carried by the durable gate fence as well as the in-process
/// gate. Store implementations can therefore enforce admission with their own
/// transactional clock while the service proves that the same authority was
/// reviewed in configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorshipRuntimeLimits {
    pub default_policy_expiry_seconds: u64,
    pub maximum_policy_expiry_seconds: u64,
    pub maximum_active_policies_per_sponsor: u32,
    pub launch_rate_window_seconds: u64,
    pub maximum_launches_per_sponsor_per_window: u32,
    pub maximum_per_launch_quota: SponsoredQuota,
    pub maximum_policy_quota: SponsoredQuota,
    pub maximum_requests_per_minute: u32,
    pub maximum_per_launch_budget_micros: u64,
    pub maximum_policy_budget_micros: u64,
    pub maximum_concurrent_launches: u32,
}

impl SponsorshipRuntimeLimits {
    pub fn disabled() -> Self {
        Self {
            default_policy_expiry_seconds: 0,
            maximum_policy_expiry_seconds: 0,
            maximum_active_policies_per_sponsor: 0,
            launch_rate_window_seconds: 0,
            maximum_launches_per_sponsor_per_window: 0,
            maximum_per_launch_quota: SponsoredQuota::default(),
            maximum_policy_quota: SponsoredQuota::default(),
            maximum_requests_per_minute: 0,
            maximum_per_launch_budget_micros: 0,
            maximum_policy_budget_micros: 0,
            maximum_concurrent_launches: 0,
        }
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.default_policy_expiry_seconds == 0
            || self.maximum_policy_expiry_seconds == 0
            || self.default_policy_expiry_seconds > self.maximum_policy_expiry_seconds
            || self.maximum_active_policies_per_sponsor == 0
            || self.launch_rate_window_seconds == 0
            || self.maximum_launches_per_sponsor_per_window == 0
            || !self.maximum_per_launch_quota.is_nonzero()
            || !self.maximum_policy_quota.is_nonzero()
            || !self
                .maximum_per_launch_quota
                .fits_within(self.maximum_policy_quota)
            || self.maximum_requests_per_minute == 0
            || self.maximum_per_launch_budget_micros == 0
            || self.maximum_policy_budget_micros == 0
            || self.maximum_per_launch_budget_micros > self.maximum_policy_budget_micros
            || self.maximum_concurrent_launches == 0
        {
            return Err(TriggerValidationError::field(
                "sponsorship_runtime_limits",
                "must contain positive, internally ordered deployment ceilings",
            ));
        }
        Ok(())
    }

    /// Resolve an omitted publisher expiry from one service-clock observation.
    ///
    /// The resolved timestamp is persisted as ordinary policy data. Stores
    /// still recheck it against their own authoritative clock before admission.
    pub fn resolve_policy_expiry(
        &self,
        requested: Option<String>,
        now: &str,
    ) -> Result<String, TriggerValidationError> {
        self.validate()?;
        validate_timestamp("sponsorship_policy_observed_at", now)?;
        if let Some(requested) = requested {
            validate_timestamp("sponsorship_policy.expires_at", &requested)?;
            return Ok(requested);
        }

        let now_seconds = crate::seconds_from_rfc3339(now).ok_or_else(|| {
            TriggerValidationError::field(
                "sponsorship_policy_observed_at",
                "must be a canonical timestamp",
            )
        })?;
        let expires_seconds = now_seconds
            .checked_add(self.default_policy_expiry_seconds)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "sponsorship_policy.expires_at",
                    "deployment default expiry overflowed",
                )
            })?;
        let expires_at = std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(expires_seconds))
            .map(crate::rfc3339_at)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "sponsorship_policy.expires_at",
                    "deployment default expiry overflowed",
                )
            })?;
        validate_timestamp("sponsorship_policy.expires_at", &expires_at)?;
        Ok(expires_at)
    }

    /// Prove every publisher-selected policy dimension against deployment
    /// authority at one already-validated clock observation.
    pub fn require_policy_at(
        &self,
        policy: &SponsorshipPolicy,
        now: &str,
    ) -> Result<(), TriggerValidationError> {
        self.validate()?;
        policy.validate()?;
        validate_timestamp("sponsorship_policy_observed_at", now)?;
        let now_seconds = crate::seconds_from_rfc3339(now).ok_or_else(|| {
            TriggerValidationError::field(
                "sponsorship_policy_observed_at",
                "must be a canonical timestamp",
            )
        })?;
        let expires_seconds = crate::seconds_from_rfc3339(&policy.expires_at).ok_or_else(|| {
            TriggerValidationError::field(
                "sponsorship_policy.expires_at",
                "must be a canonical timestamp",
            )
        })?;
        let latest_expiry = now_seconds
            .checked_add(self.maximum_policy_expiry_seconds)
            .ok_or_else(|| {
                TriggerValidationError::field(
                    "sponsorship_policy.expires_at",
                    "deployment expiry authority overflowed",
                )
            })?;
        if expires_seconds <= now_seconds
            || expires_seconds > latest_expiry
            || !policy
                .per_launch_quota
                .fits_within(self.maximum_per_launch_quota)
            || !policy
                .aggregate_quota
                .fits_within(self.maximum_policy_quota)
            || policy.per_launch_budget_micros > self.maximum_per_launch_budget_micros
            || policy.aggregate_budget_micros > self.maximum_policy_budget_micros
            || policy.max_requests_per_minute > self.maximum_requests_per_minute
            || policy.max_concurrent_launches > self.maximum_concurrent_launches
        {
            return Err(TriggerValidationError::field(
                "sponsorship_policy",
                "exceeds deployment-owned expiry, quota, budget, rate, or concurrency authority",
            ));
        }
        Ok(())
    }
}

impl SponsoredQuota {
    pub(super) fn is_nonzero(self) -> bool {
        self.requests > 0 && self.input_tokens > 0 && self.output_tokens > 0
    }

    pub fn checked_add(self, other: Self) -> Option<Self> {
        Some(Self {
            requests: self.requests.checked_add(other.requests)?,
            input_tokens: self.input_tokens.checked_add(other.input_tokens)?,
            output_tokens: self.output_tokens.checked_add(other.output_tokens)?,
        })
    }

    pub fn fits_within(self, ceiling: Self) -> bool {
        self.requests <= ceiling.requests
            && self.input_tokens <= ceiling.input_tokens
            && self.output_tokens <= ceiling.output_tokens
    }

    pub fn checked_sub(self, other: Self) -> Option<Self> {
        Some(Self {
            requests: self.requests.checked_sub(other.requests)?,
            input_tokens: self.input_tokens.checked_sub(other.input_tokens)?,
            output_tokens: self.output_tokens.checked_sub(other.output_tokens)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredProviderPolicy {
    pub provider: String,
    pub provider_policy_key: String,
    #[serde(default)]
    pub allowed_models: BTreeSet<String>,
    #[serde(default)]
    pub allowed_resources: BTreeSet<String>,
}

impl SponsoredProviderPolicy {
    fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_policy_token("provider_policy.provider", &self.provider)?;
        validate_policy_token(
            "provider_policy.provider_policy_key",
            &self.provider_policy_key,
        )?;
        if self.allowed_models.is_empty() || self.allowed_models.len() > MAX_SPONSORED_MODELS {
            return Err(TriggerValidationError::field(
                "provider_policy.allowed_models",
                "must contain between 1 and 32 distinct models",
            ));
        }
        if self.allowed_resources.is_empty()
            || self.allowed_resources.len() > MAX_SPONSORED_RESOURCES
        {
            return Err(TriggerValidationError::field(
                "provider_policy.allowed_resources",
                "must contain between 1 and 32 distinct resources",
            ));
        }
        for model in &self.allowed_models {
            validate_policy_token("provider_policy.allowed_models", model)?;
        }
        for resource in &self.allowed_resources {
            validate_policy_token("provider_policy.allowed_resources", resource)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SponsorshipPolicyState {
    Active,
    Suspended,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorshipPolicy {
    pub schema_version: u32,
    pub id: SponsorshipPolicyId,
    pub tenant_id: String,
    pub owner: SponsorBillingOwner,
    pub credential_id: SponsoredCredentialId,
    pub provider_policy: SponsoredProviderPolicy,
    pub per_launch_quota: SponsoredQuota,
    pub aggregate_quota: SponsoredQuota,
    pub per_launch_budget_micros: u64,
    pub aggregate_budget_micros: u64,
    pub max_requests_per_minute: u32,
    pub max_concurrent_launches: u32,
    pub expires_at: String,
    pub state: SponsorshipPolicyState,
    pub generation: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

/// Owner-safe policy projection. Credential locators, owner subjects, and
/// provider authority never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorshipPolicyView {
    pub id: SponsorshipPolicyId,
    pub tenant_id: String,
    pub sponsor_display_name: String,
    pub provider: String,
    pub provider_policy_key: String,
    pub allowed_models: BTreeSet<String>,
    pub allowed_resources: BTreeSet<String>,
    pub per_launch_quota: SponsoredQuota,
    pub aggregate_quota: SponsoredQuota,
    pub per_launch_budget_micros: u64,
    pub aggregate_budget_micros: u64,
    pub max_requests_per_minute: u32,
    pub max_concurrent_launches: u32,
    pub expires_at: String,
    pub state: SponsorshipPolicyState,
    pub generation: u64,
    pub usage: SponsorshipPolicyUsage,
    pub updated_at: String,
}

impl SponsorshipPolicyView {
    pub fn from_policy(policy: &SponsorshipPolicy, usage: SponsorshipPolicyUsage) -> Self {
        Self {
            id: policy.id.clone(),
            tenant_id: policy.tenant_id.clone(),
            sponsor_display_name: policy.owner.display_name().to_string(),
            provider: policy.provider_policy.provider.clone(),
            provider_policy_key: policy.provider_policy.provider_policy_key.clone(),
            allowed_models: policy.provider_policy.allowed_models.clone(),
            allowed_resources: policy.provider_policy.allowed_resources.clone(),
            per_launch_quota: policy.per_launch_quota,
            aggregate_quota: policy.aggregate_quota,
            per_launch_budget_micros: policy.per_launch_budget_micros,
            aggregate_budget_micros: policy.aggregate_budget_micros,
            max_requests_per_minute: policy.max_requests_per_minute,
            max_concurrent_launches: policy.max_concurrent_launches,
            expires_at: policy.expires_at.clone(),
            state: policy.state,
            generation: policy.generation,
            usage,
            updated_at: policy.updated_at.clone(),
        }
    }
}

impl SponsorshipPolicy {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.schema_version != SPONSORSHIP_SCHEMA_VERSION || self.generation == 0 {
            return Err(TriggerValidationError::field(
                "sponsorship_policy",
                "has an unsupported schema or zero generation",
            ));
        }
        SponsorshipPolicyId::parse(self.id.0.clone())?;
        SponsoredCredentialId::parse(self.credential_id.0.clone())?;
        validate_identifier("sponsorship_policy.tenant_id", &self.tenant_id, 255)?;
        self.owner.validate()?;
        self.provider_policy.validate()?;
        if !self.per_launch_quota.is_nonzero()
            || !self.aggregate_quota.is_nonzero()
            || !self.per_launch_quota.fits_within(self.aggregate_quota)
        {
            return Err(TriggerValidationError::field(
                "sponsorship_policy.quota",
                "per-Launch and aggregate quota must be nonzero and ordered",
            ));
        }
        if self.per_launch_budget_micros == 0
            || self.aggregate_budget_micros == 0
            || self.per_launch_budget_micros > self.aggregate_budget_micros
        {
            return Err(TriggerValidationError::field(
                "sponsorship_policy.budget",
                "per-Launch and aggregate budgets must be nonzero and ordered",
            ));
        }
        if self.max_requests_per_minute == 0 || self.max_concurrent_launches == 0 {
            return Err(TriggerValidationError::field(
                "sponsorship_policy.limits",
                "rate and concurrent Launch limits must be positive",
            ));
        }
        validate_timestamp("sponsorship_policy.expires_at", &self.expires_at)?;
        validate_timestamp("sponsorship_policy.created_at", &self.created_at)?;
        validate_timestamp("sponsorship_policy.updated_at", &self.updated_at)?;
        if !timestamp_before(&self.created_at, &self.expires_at) {
            return Err(TriggerValidationError::field(
                "sponsorship_policy.expires_at",
                "must be later than creation",
            ));
        }
        if !timestamp_not_after(&self.created_at, &self.updated_at) {
            return Err(TriggerValidationError::field(
                "sponsorship_policy.updated_at",
                "must not precede creation",
            ));
        }
        if (self.state == SponsorshipPolicyState::Revoked) != self.revoked_at.is_some() {
            return Err(TriggerValidationError::field(
                "sponsorship_policy.revoked_at",
                "must exist exactly for a revoked policy",
            ));
        }
        if let Some(revoked_at) = &self.revoked_at {
            validate_timestamp("sponsorship_policy.revoked_at", revoked_at)?;
            if !timestamp_not_after(&self.created_at, revoked_at)
                || !timestamp_not_after(revoked_at, &self.updated_at)
            {
                return Err(TriggerValidationError::field(
                    "sponsorship_policy.revoked_at",
                    "must fall between creation and the latest update",
                ));
            }
        }
        Ok(())
    }

    pub fn is_available_at(&self, now: &str) -> bool {
        self.state == SponsorshipPolicyState::Active
            && crate::seconds_from_rfc3339(now).is_some_and(|now| {
                crate::seconds_from_rfc3339(&self.expires_at).is_some_and(|expires| now < expires)
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCredentialRoute {
    Contributor,
    SponsoredGrant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialRouteDisclosure {
    /// GitHub remains the contributor's selected NyxID connection.
    pub github: CredentialRoute,
    /// Model/provider billing is the independently reviewed route.
    pub model_provider: ModelCredentialRoute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredGrantTerms {
    pub schema_version: u32,
    pub policy_id: SponsorshipPolicyId,
    pub policy_generation: u64,
    pub sponsor_display_name: String,
    pub provider: String,
    pub allowed_models: BTreeSet<String>,
    pub allowed_resources: BTreeSet<String>,
    pub per_launch_quota: SponsoredQuota,
    pub aggregate_remaining_quota: SponsoredQuota,
    pub per_launch_budget_micros: u64,
    pub aggregate_remaining_budget_micros: u64,
    pub max_requests_per_minute: u32,
    pub max_concurrent_launches: u32,
    pub available_launches: u32,
    pub expires_at: String,
    pub credential_route: CredentialRouteDisclosure,
    pub digest: String,
}

/// Contributor review contract binding the visible terms to the exact Launch
/// configuration and current policy generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredTermsReview {
    pub terms: SponsoredGrantTerms,
    pub launch_snapshot_digest: String,
}

impl SponsoredGrantTerms {
    pub fn from_policy(
        policy: &SponsorshipPolicy,
        usage: &SponsorshipPolicyUsage,
    ) -> Result<Self, TriggerValidationError> {
        policy.validate()?;
        let exposure = usage.exposure()?;
        let aggregate_remaining_quota = SponsoredQuota {
            requests: policy
                .aggregate_quota
                .requests
                .saturating_sub(exposure.quota.requests),
            input_tokens: policy
                .aggregate_quota
                .input_tokens
                .saturating_sub(exposure.quota.input_tokens),
            output_tokens: policy
                .aggregate_quota
                .output_tokens
                .saturating_sub(exposure.quota.output_tokens),
        };
        let mut terms = Self {
            schema_version: SPONSORSHIP_SCHEMA_VERSION,
            policy_id: policy.id.clone(),
            policy_generation: policy.generation,
            sponsor_display_name: policy.owner.display_name().to_string(),
            provider: policy.provider_policy.provider.clone(),
            allowed_models: policy.provider_policy.allowed_models.clone(),
            allowed_resources: policy.provider_policy.allowed_resources.clone(),
            per_launch_quota: policy.per_launch_quota,
            aggregate_remaining_quota,
            per_launch_budget_micros: policy.per_launch_budget_micros,
            aggregate_remaining_budget_micros: policy
                .aggregate_budget_micros
                .saturating_sub(exposure.spend_micros),
            max_requests_per_minute: policy.max_requests_per_minute,
            max_concurrent_launches: policy.max_concurrent_launches,
            available_launches: policy
                .max_concurrent_launches
                .saturating_sub(usage.active_launches),
            expires_at: policy.expires_at.clone(),
            credential_route: CredentialRouteDisclosure {
                github: CredentialRoute::ContributorConnection,
                model_provider: ModelCredentialRoute::SponsoredGrant,
            },
            digest: String::new(),
        };
        terms.digest = terms.calculate_digest()?;
        Ok(terms)
    }

    /// Canonical terms identity without mutable remaining-capacity fields.
    ///
    /// `calculate_digest` deliberately excludes aggregate remaining quota,
    /// budget, and currently available Launch slots. A Launch authority can
    /// therefore bind the policy terms without turning unrelated concurrent
    /// usage into a false acceptance change.
    pub fn digest_from_policy(
        policy: &SponsorshipPolicy,
    ) -> Result<String, TriggerValidationError> {
        Ok(Self::from_policy(
            policy,
            &SponsorshipPolicyUsage {
                consumed: SponsoredUsageSummary::default(),
                reserved: SponsoredUsageSummary::default(),
                active_launches: 0,
            },
        )?
        .digest)
    }

    pub fn calculate_digest(&self) -> Result<String, TriggerValidationError> {
        let bytes = serde_json::to_vec(&(
            "cma-sponsored-terms-v1",
            (
                self.schema_version,
                &self.policy_id,
                self.policy_generation,
                &self.sponsor_display_name,
                &self.provider,
                &self.allowed_models,
                &self.allowed_resources,
                self.per_launch_quota,
            ),
            (
                self.per_launch_budget_micros,
                self.max_requests_per_minute,
                self.max_concurrent_launches,
                &self.expires_at,
                &self.credential_route,
            ),
        ))
        .map_err(|_| TriggerValidationError::new("sponsored terms cannot be encoded"))?;
        Ok(lower_hex(Sha256::digest(bytes)))
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.schema_version != SPONSORSHIP_SCHEMA_VERSION
            || self.digest != self.calculate_digest()?
        {
            return Err(TriggerValidationError::field(
                "sponsored_terms",
                "has an unsupported schema or mismatched digest",
            ));
        }
        validate_display_name(
            "sponsored_terms.sponsor_display_name",
            &self.sponsor_display_name,
        )?;
        validate_policy_token("sponsored_terms.provider", &self.provider)?;
        validate_timestamp("sponsored_terms.expires_at", &self.expires_at)?;
        SponsorshipPolicyId::parse(self.policy_id.0.clone())?;
        if self.policy_generation == 0
            || self.allowed_models.is_empty()
            || self.allowed_models.len() > MAX_SPONSORED_MODELS
            || self.allowed_resources.is_empty()
            || self.allowed_resources.len() > MAX_SPONSORED_RESOURCES
            || !self.per_launch_quota.is_nonzero()
            || self.per_launch_budget_micros == 0
            || self.max_requests_per_minute == 0
            || self.max_concurrent_launches == 0
            || self.available_launches > self.max_concurrent_launches
        {
            return Err(TriggerValidationError::field(
                "sponsored_terms",
                "contains invalid policy generation, limits, models, resources, quota, or budget",
            ));
        }
        for model in &self.allowed_models {
            validate_policy_token("sponsored_terms.allowed_models", model)?;
        }
        for resource in &self.allowed_resources {
            validate_policy_token("sponsored_terms.allowed_resources", resource)?;
        }
        if self.credential_route.github != CredentialRoute::ContributorConnection
            || self.credential_route.model_provider != ModelCredentialRoute::SponsoredGrant
        {
            return Err(TriggerValidationError::field(
                "sponsored_terms.credential_route",
                "must keep GitHub contributor-owned and model spend sponsored",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredGrantAcceptance {
    pub launch_id: LaunchId,
    pub contributor: SubjectId,
    pub revision_id: TriggerRevisionId,
    pub revision_digest: String,
    pub policy_id: SponsorshipPolicyId,
    pub policy_generation: u64,
    pub terms_digest: String,
    /// Exact effective Launch configuration, pricing, runtime, and sponsored
    /// terms the contributor reviewed. An override changes this digest and
    /// requires another acceptance before Start.
    pub launch_snapshot_digest: String,
    pub accepted_at: String,
    pub idempotency_key_hash: String,
    pub digest: String,
}

impl SponsoredGrantAcceptance {
    pub fn calculate_digest(&self) -> Result<String, TriggerValidationError> {
        canonical_digest(&(
            "cma-sponsored-acceptance-v2",
            &self.launch_id,
            &self.contributor,
            &self.revision_id,
            &self.revision_digest,
            &self.policy_id,
            self.policy_generation,
            &self.terms_digest,
            &self.launch_snapshot_digest,
            &self.accepted_at,
            &self.idempotency_key_hash,
        ))
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        LaunchId::parse(self.launch_id.0.clone())?;
        TriggerRevisionId::parse(self.revision_id.0.clone())?;
        validate_identifier("acceptance.contributor", &self.contributor.0, 255)?;
        validate_digest("acceptance.revision_digest", &self.revision_digest)?;
        validate_digest("acceptance.terms_digest", &self.terms_digest)?;
        validate_digest(
            "acceptance.launch_snapshot_digest",
            &self.launch_snapshot_digest,
        )?;
        SponsorshipOperationalPseudonym::parse(self.idempotency_key_hash.clone())?;
        validate_digest("acceptance.digest", &self.digest)?;
        validate_timestamp("acceptance.accepted_at", &self.accepted_at)?;
        if self.policy_generation == 0 || self.digest != self.calculate_digest()? {
            return Err(TriggerValidationError::field(
                "acceptance",
                "must have a positive policy generation and canonical digest",
            ));
        }
        Ok(())
    }
}

/// Sponsorship-relevant facts read from the durable Launch record.
///
/// Callers may repeat these values in a request, but the sponsorship store
/// must compare this entire authority (including generation and digest) to the
/// Launch document in the same transaction that accepts terms, commits a
/// grant, or reserves spend. It is therefore a compare-and-set fence, not a
/// bearer token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SponsoredLaunchState {
    TermsReview,
    Starting,
    Active,
    Recoverable,
    Closed,
}

impl SponsoredLaunchState {
    fn as_str(self) -> &'static str {
        match self {
            Self::TermsReview => "terms_review",
            Self::Starting => "starting",
            Self::Active => "active",
            Self::Recoverable => "recoverable",
            Self::Closed => "closed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredLaunchAuthority {
    pub tenant_id: String,
    pub launch_id: LaunchId,
    pub contributor: SubjectId,
    pub revision_id: TriggerRevisionId,
    pub revision_digest: String,
    pub launch_snapshot_digest: String,
    pub policy_id: SponsorshipPolicyId,
    pub session_id: Option<SessionId>,
    pub state: SponsoredLaunchState,
    pub generation: u64,
    pub digest: String,
}

impl SponsoredLaunchAuthority {
    /// Derive the only sponsorship authority projection accepted from a
    /// durable Trigger Launch.
    pub fn from_launch(
        launch: &Launch,
        policy: &SponsorshipPolicy,
    ) -> Result<Self, TriggerValidationError> {
        let commercial = &launch.revision.content.commercial_policy;
        if commercial.billing_mode != BillingMode::Sponsored
            || commercial.policy_reference != CMA_TRIGGER_SPONSORED_POLICY_V1
        {
            return Err(TriggerValidationError::field(
                "commercial_policy",
                "does not name the sponsored grant contract",
            ));
        }
        let binding = commercial.sponsorship.as_ref().ok_or_else(|| {
            TriggerValidationError::field(
                "commercial_policy.sponsorship",
                "is required for sponsored billing",
            )
        })?;
        if binding.policy_id != policy.id || binding.tenant_id != policy.tenant_id {
            return Err(TriggerValidationError::field(
                "commercial_policy.sponsorship",
                "does not match the durable sponsorship policy authority",
            ));
        }
        let terms_digest = SponsoredGrantTerms::digest_from_policy(policy)?;
        let launch_snapshot_digest = canonical_digest(&(
            "cma-sponsored-launch-review-v1",
            &launch.effective.digest,
            &binding.tenant_id,
            &binding.policy_id,
            policy.generation,
            &terms_digest,
        ))?;
        let state = match launch.state {
            LaunchState::Draft if launch.started_at.is_none() => SponsoredLaunchState::TermsReview,
            LaunchState::AuthRequired if launch.started_at.is_none() => {
                SponsoredLaunchState::TermsReview
            }
            LaunchState::Preflight
            | LaunchState::ForkPending
            | LaunchState::ForkReady
            | LaunchState::BranchPending
            | LaunchState::WorkspacePending
            | LaunchState::SandboxPending
            | LaunchState::SessionPending
            | LaunchState::PromptPending => SponsoredLaunchState::Starting,
            LaunchState::Ready
            | LaunchState::Contributing
            | LaunchState::PrPending
            | LaunchState::PrOpen => SponsoredLaunchState::Active,
            LaunchState::AuthRequired | LaunchState::FailedRetryable => {
                SponsoredLaunchState::Recoverable
            }
            LaunchState::Completed | LaunchState::FailedTerminal | LaunchState::Cancelled => {
                SponsoredLaunchState::Closed
            }
            LaunchState::Draft => {
                return Err(TriggerValidationError::field(
                    "launch.started_at",
                    "a started Launch cannot return to draft sponsorship review",
                ))
            }
        };
        // Launch worker leases, effect receipts, and retry bookkeeping all advance
        // `revision_number`. None of those changes sponsorship authority, and
        // binding grant activation to that global CAS would make an ambiguous
        // provider operation impossible to reconcile after the worker persisted
        // its retry state. Session attachment is the sponsorship-relevant
        // authority transition: before it there is no spend authority, and after
        // it the exact deterministic session remains stable for the Launch.
        let generation = launch
            .session
            .as_ref()
            .map(|session| {
                session.runtime_generation.checked_add(1).ok_or_else(|| {
                    TriggerValidationError::field(
                        "launch.session.runtime_generation",
                        "is exhausted",
                    )
                })
            })
            .transpose()?
            .unwrap_or(1);
        let mut authority = Self {
            tenant_id: binding.tenant_id.clone(),
            launch_id: launch.id.clone(),
            contributor: launch.owner.clone(),
            revision_id: launch.revision.id.clone(),
            revision_digest: launch.revision_digest.clone(),
            launch_snapshot_digest,
            policy_id: binding.policy_id.clone(),
            session_id: launch
                .session
                .as_ref()
                .map(|session| session.session_id.clone()),
            state,
            generation,
            digest: String::new(),
        };
        authority.digest = authority.calculate_digest()?;
        authority.validate()?;
        Ok(authority)
    }

    pub fn calculate_digest(&self) -> Result<String, TriggerValidationError> {
        canonical_digest(&(
            "cma-sponsored-launch-authority-v1",
            &self.tenant_id,
            &self.launch_id,
            &self.contributor,
            &self.revision_id,
            &self.revision_digest,
            &self.launch_snapshot_digest,
            &self.policy_id,
            &self.session_id,
            self.state.as_str(),
            self.generation,
        ))
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_identifier("launch_authority.tenant_id", &self.tenant_id, 255)?;
        LaunchId::parse(self.launch_id.0.clone())?;
        TriggerRevisionId::parse(self.revision_id.0.clone())?;
        SponsorshipPolicyId::parse(self.policy_id.0.clone())?;
        validate_identifier("launch_authority.contributor", &self.contributor.0, 255)?;
        validate_digest("launch_authority.revision_digest", &self.revision_digest)?;
        validate_digest(
            "launch_authority.launch_snapshot_digest",
            &self.launch_snapshot_digest,
        )?;
        if let Some(session_id) = &self.session_id {
            validate_identifier("launch_authority.session_id", &session_id.0, 255)?;
        }
        if self.generation == 0 || self.digest != self.calculate_digest()? {
            return Err(TriggerValidationError::field(
                "launch_authority",
                "has a zero generation or mismatched canonical digest",
            ));
        }
        Ok(())
    }

    pub fn require_terms_review(&self) -> Result<(), SponsorshipError> {
        if self.state != SponsoredLaunchState::TermsReview || self.session_id.is_some() {
            return Err(SponsorshipError::ScopeDenied);
        }
        Ok(())
    }

    pub fn require_activation(&self) -> Result<&SessionId, SponsorshipError> {
        if self.state != SponsoredLaunchState::Starting {
            return Err(SponsorshipError::ScopeDenied);
        }
        self.session_id
            .as_ref()
            .ok_or(SponsorshipError::ScopeDenied)
    }

    pub fn require_spend(&self) -> Result<&SessionId, SponsorshipError> {
        if !matches!(
            self.state,
            SponsoredLaunchState::Active | SponsoredLaunchState::Recoverable
        ) {
            return Err(SponsorshipError::ScopeDenied);
        }
        self.session_id
            .as_ref()
            .ok_or(SponsorshipError::ScopeDenied)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipGateFence {
    pub configuration_digest: String,
    pub epoch: u64,
    pub kill_switch_engaged: bool,
    pub trusted_provider_policy_key: String,
    pub runtime_limits: SponsorshipRuntimeLimits,
}

impl SponsorshipGateFence {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_digest(
            "sponsorship_gate.configuration_digest",
            &self.configuration_digest,
        )?;
        if self.epoch == 0 {
            return Err(TriggerValidationError::field(
                "sponsorship_gate.epoch",
                "must be positive",
            ));
        }
        validate_policy_token(
            "sponsorship_gate.trusted_provider_policy_key",
            &self.trusted_provider_policy_key,
        )?;
        self.runtime_limits.validate()?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredPriceQuote {
    pub provider: String,
    pub model: String,
    pub resource: String,
    pub maximum: SponsoredUsageSummary,
    pub pricing_generation: u64,
    pub digest: String,
}

impl SponsoredPriceQuote {
    pub fn calculate_digest(&self) -> Result<String, TriggerValidationError> {
        canonical_digest(&(
            "cma-sponsored-price-quote-v1",
            &self.provider,
            &self.model,
            &self.resource,
            &self.maximum,
            self.pricing_generation,
        ))
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_policy_token("price_quote.provider", &self.provider)?;
        validate_policy_token("price_quote.model", &self.model)?;
        validate_policy_token("price_quote.resource", &self.resource)?;
        if self.pricing_generation == 0
            || !self.maximum.quota.is_nonzero()
            || self.maximum.spend_micros == 0
            || self.digest != self.calculate_digest()?
        {
            return Err(TriggerValidationError::field(
                "price_quote",
                "must contain positive canonical quota, spend, and pricing generation",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SponsoredGrantState {
    Active,
    Exhausted,
    Expired,
    RevocationPending,
    Revoked,
    Closed,
    Suspended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderGrantAuthority {
    pub provider_grant_id_digest: String,
    pub provider_state_version: u64,
    pub expires_at: String,
    pub revocation_handle_digest: String,
    pub issued_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InertGrantCapability {
    pub grant_id: SponsoredGrantId,
    pub grant_generation: u64,
    pub policy_generation: u64,
    pub launch_id: LaunchId,
    pub session_id: SessionId,
    pub expires_at: String,
    pub fingerprint: String,
}

impl InertGrantCapability {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        SponsoredGrantId::parse(self.grant_id.0.clone())?;
        LaunchId::parse(self.launch_id.0.clone())?;
        validate_identifier("capability.session_id", &self.session_id.0, 255)?;
        validate_timestamp("capability.expires_at", &self.expires_at)?;
        validate_digest("capability.fingerprint", &self.fingerprint)?;
        if self.grant_generation == 0 || self.policy_generation == 0 {
            return Err(TriggerValidationError::field(
                "capability.generation",
                "must be positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredUsageSummary {
    pub quota: SponsoredQuota,
    pub spend_micros: u64,
}

impl SponsoredUsageSummary {
    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        Some(Self {
            quota: self.quota.checked_add(other.quota)?,
            spend_micros: self.spend_micros.checked_add(other.spend_micros)?,
        })
    }

    pub fn checked_sub(&self, other: &Self) -> Option<Self> {
        Some(Self {
            quota: self.quota.checked_sub(other.quota)?,
            spend_micros: self.spend_micros.checked_sub(other.spend_micros)?,
        })
    }

    pub fn fits_within(&self, ceiling: &Self) -> bool {
        self.quota.fits_within(ceiling.quota) && self.spend_micros <= ceiling.spend_micros
    }

    /// Sponsored grants fail closed when any mandatory quota or budget
    /// dimension reaches its allocation. Waiting for every independent
    /// dimension to reach the ceiling would permit spend after one bound was
    /// already exhausted.
    pub fn reaches_any_limit(&self, allocation: &Self) -> bool {
        self.quota.requests >= allocation.quota.requests
            || self.quota.input_tokens >= allocation.quota.input_tokens
            || self.quota.output_tokens >= allocation.quota.output_tokens
            || self.spend_micros >= allocation.spend_micros
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredGrant {
    pub schema_version: u32,
    pub id: SponsoredGrantId,
    pub tenant_id: String,
    pub policy_id: SponsorshipPolicyId,
    pub policy_generation: u64,
    /// Root-credential generation that issued this grant. Successful root
    /// rotation preserves independently scoped grants, so this is provenance;
    /// credential state, not current-generation equality, is the live spend
    /// revocation fence.
    pub credential_generation: u64,
    pub trigger_revision_id: TriggerRevisionId,
    pub trigger_revision_digest: String,
    pub launch_id: LaunchId,
    pub session_id: SessionId,
    pub contributor: SubjectId,
    /// Policy controller fixed when the grant is issued. Persistence may
    /// denormalize this for sponsor listings, but every read must retain this
    /// canonical binding to the policy owner.
    pub sponsor_subject: SubjectId,
    pub terms_digest: String,
    pub acceptance: SponsoredGrantAcceptance,
    pub authority: ProviderGrantAuthority,
    pub capability: InertGrantCapability,
    pub state: SponsoredGrantState,
    /// The exact quota and budget atomically carved out of the aggregate
    /// policy allowance when this grant committed.
    pub allocation: SponsoredUsageSummary,
    pub usage: SponsoredUsageSummary,
    pub generation: u64,
    pub created_at: String,
    pub updated_at: String,
    pub revoked_at: Option<String>,
    pub pending_revocation: Option<PendingProviderOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProviderOperationKind {
    RotateCredential,
    RevokeCredential,
    RevokeGrant,
    CloseGrant,
    RevokeUncommittedGrant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderOperationPhase {
    Prepared,
    Attempting,
    Ambiguous,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingProviderOperation {
    pub kind: ProviderOperationKind,
    pub phase: ProviderOperationPhase,
    pub provider_request_id: String,
    /// Digest of the caller or internal request that owns this mutation. This
    /// survives phase changes so a retry cannot substitute a different payload.
    pub client_request_id_digest: String,
    pub payload_digest: String,
    pub requested_at: String,
    pub attempted_at: Option<String>,
}

impl PendingProviderOperation {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_digest(
            "provider_operation.provider_request_id",
            &self.provider_request_id,
        )?;
        SponsorshipOperationalPseudonym::parse(self.client_request_id_digest.clone())?;
        validate_digest("provider_operation.payload_digest", &self.payload_digest)?;
        validate_timestamp("provider_operation.requested_at", &self.requested_at)?;
        if let Some(attempted_at) = &self.attempted_at {
            validate_timestamp("provider_operation.attempted_at", attempted_at)?;
            if attempted_at < &self.requested_at {
                return Err(TriggerValidationError::field(
                    "provider_operation.attempted_at",
                    "must not precede the durable request",
                ));
            }
        }
        if (self.phase == ProviderOperationPhase::Prepared) != self.attempted_at.is_none() {
            return Err(TriggerValidationError::field(
                "provider_operation.phase",
                "must be prepared exactly until its first provider attempt is fenced",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletedProviderOperationOutcome {
    Confirmed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedProviderOperation {
    pub kind: ProviderOperationKind,
    pub provider_request_id: String,
    pub client_request_id_digest: String,
    pub payload_digest: String,
    pub outcome: CompletedProviderOperationOutcome,
    pub completed_at: String,
}

impl CompletedProviderOperation {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        validate_digest(
            "completed_provider_operation.provider_request_id",
            &self.provider_request_id,
        )?;
        SponsorshipOperationalPseudonym::parse(self.client_request_id_digest.clone())?;
        validate_digest(
            "completed_provider_operation.payload_digest",
            &self.payload_digest,
        )?;
        validate_timestamp(
            "completed_provider_operation.completed_at",
            &self.completed_at,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantActivationState {
    AwaitingBilling,
    BillingAttempting,
    BillingAmbiguous,
    AwaitingProvider,
    ProviderAttempting,
    ProviderAmbiguous,
    ProviderConfirmed,
    CleanupPending,
    Cleaned,
    Denied,
    Committed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantActivationDenial {
    BillingRejected,
    ProviderRejected,
    AuthorityInvalidated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantActivationLease {
    pub holder: String,
    pub acquired_at: String,
    pub expires_at: String,
}

/// Durable intent inserted before the read-only billing eligibility observation
/// and before any provider effect.
///
/// `provider_request_id` is a keyed operational pseudonym derived from
/// immutable Launch/policy identity, not from a caller's retry key. Every
/// takeover therefore reconciles the same frozen external operation.
/// `provider_receipt` is internal persistence data and has no serialization
/// implementation; durable adapters encrypt its two provider handles before
/// storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantActivationIntent {
    pub launch_id: LaunchId,
    pub session_id: SessionId,
    pub policy_id: SponsorshipPolicyId,
    pub policy_generation: u64,
    pub credential_generation: u64,
    pub launch_authority_digest: String,
    pub acceptance_digest: String,
    pub gate_fence: SponsorshipGateFence,
    pub allocation: SponsoredUsageSummary,
    pub request_fingerprint: String,
    pub billing_request_id: String,
    pub provider_request_id: String,
    pub state: GrantActivationState,
    pub denial: Option<GrantActivationDenial>,
    /// Ownership marker for the aggregate allocation and concurrency slot.
    /// Persistence creates this hold before billing or provider observation;
    /// commit transfers the already-counted hold to the grant and clears this
    /// marker without incrementing policy usage a second time.
    pub capacity_held: bool,
    pub revision: u64,
    pub lease: Option<GrantActivationLease>,
    pub provider_receipt: Option<ProviderGrantReceipt>,
    pub cleanup_operation: Option<PendingProviderOperation>,
    pub created_at: String,
    pub updated_at: String,
}

impl GrantActivationIntent {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        LaunchId::parse(self.launch_id.0.clone())?;
        validate_identifier("grant_activation.session_id", &self.session_id.0, 255)?;
        SponsorshipPolicyId::parse(self.policy_id.0.clone())?;
        validate_digest(
            "grant_activation.launch_authority_digest",
            &self.launch_authority_digest,
        )?;
        validate_digest(
            "grant_activation.acceptance_digest",
            &self.acceptance_digest,
        )?;
        validate_digest(
            "grant_activation.request_fingerprint",
            &self.request_fingerprint,
        )?;
        SponsorshipOperationalPseudonym::parse(self.billing_request_id.clone())?;
        SponsorshipOperationalPseudonym::parse(self.provider_request_id.clone())?;
        validate_timestamp("grant_activation.created_at", &self.created_at)?;
        validate_timestamp("grant_activation.updated_at", &self.updated_at)?;
        if !timestamp_not_after(&self.created_at, &self.updated_at) {
            return Err(TriggerValidationError::field(
                "grant_activation.updated_at",
                "must not precede creation",
            ));
        }
        self.gate_fence.validate()?;
        if self.policy_generation == 0
            || self.credential_generation == 0
            || self.revision == 0
            || !self.allocation.quota.is_nonzero()
            || self.allocation.spend_micros == 0
        {
            return Err(TriggerValidationError::field(
                "grant_activation",
                "must have positive generations, revision, quota, and budget",
            ));
        }
        if let Some(lease) = &self.lease {
            lease.validate()?;
            if !timestamp_not_after(&self.created_at, &lease.acquired_at)
                || !timestamp_not_after(&lease.acquired_at, &self.updated_at)
            {
                return Err(TriggerValidationError::field(
                    "grant_activation.lease",
                    "must contain the latest update within the activation lineage",
                ));
            }
        }
        if self.provider_receipt.is_some()
            && !matches!(
                self.state,
                GrantActivationState::ProviderAmbiguous
                    | GrantActivationState::ProviderConfirmed
                    | GrantActivationState::CleanupPending
                    | GrantActivationState::Cleaned
                    | GrantActivationState::Committed
            )
        {
            return Err(TriggerValidationError::field(
                "grant_activation.provider_receipt",
                "may only accompany ambiguous, confirmed, or committed provider issuance",
            ));
        }
        if matches!(
            self.state,
            GrantActivationState::ProviderConfirmed
                | GrantActivationState::CleanupPending
                | GrantActivationState::Cleaned
                | GrantActivationState::Committed
        ) && self.provider_receipt.is_none()
        {
            return Err(TriggerValidationError::field(
                "grant_activation.provider_receipt",
                "is required for a committed activation",
            ));
        }
        if (self.state == GrantActivationState::Denied) != self.denial.is_some() {
            return Err(TriggerValidationError::field(
                "grant_activation.denial",
                "must exist exactly for a definitive denial",
            ));
        }
        if matches!(
            self.state,
            GrantActivationState::BillingAttempting
                | GrantActivationState::BillingAmbiguous
                | GrantActivationState::ProviderAttempting
                | GrantActivationState::ProviderAmbiguous
                | GrantActivationState::ProviderConfirmed
                | GrantActivationState::CleanupPending
        ) && !self.capacity_held
        {
            return Err(TriggerValidationError::field(
                "grant_activation.capacity_held",
                "ambiguity and confirmed provider authority must retain capacity",
            ));
        }
        if matches!(
            self.state,
            GrantActivationState::Denied
                | GrantActivationState::Cleaned
                | GrantActivationState::Committed
        ) && self.capacity_held
        {
            return Err(TriggerValidationError::field(
                "grant_activation.capacity_held",
                "terminal and committed intents cannot retain activation capacity",
            ));
        }
        if (self.state == GrantActivationState::CleanupPending) != self.cleanup_operation.is_some()
        {
            return Err(TriggerValidationError::field(
                "grant_activation.cleanup_operation",
                "must exist exactly while uncommitted provider authority is being revoked",
            ));
        }
        if let Some(operation) = &self.cleanup_operation {
            operation.validate()?;
            if !operation_is_within_record(operation, &self.created_at, &self.updated_at) {
                return Err(TriggerValidationError::field(
                    "grant_activation.cleanup_operation",
                    "must fall within the activation timestamp lineage",
                ));
            }
            if operation.kind != ProviderOperationKind::RevokeUncommittedGrant {
                return Err(TriggerValidationError::field(
                    "grant_activation.cleanup_operation.kind",
                    "must revoke the uncommitted provider grant",
                ));
            }
        }
        Ok(())
    }
}

impl GrantActivationLease {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        SponsorshipOperationalPseudonym::parse(self.holder.clone())?;
        validate_timestamp("grant_activation.lease.acquired_at", &self.acquired_at)?;
        validate_timestamp("grant_activation.lease.expires_at", &self.expires_at)?;
        if self.acquired_at >= self.expires_at {
            return Err(TriggerValidationError::field(
                "grant_activation.lease.expires_at",
                "must follow acquisition",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantActivationClaim {
    pub acquired: bool,
    pub intent: GrantActivationIntent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimGrantActivation {
    pub candidate: GrantActivationIntent,
    /// Complete bounded identities for the caller request acquiring the lease.
    /// The store persists the current candidate when it creates a new lease.
    pub holder_candidates: SponsorshipOperationalPseudonymSet,
    /// Candidate identities for external operations. Existing durable values
    /// are matched across rotation and never rewritten after dispatch.
    pub billing_request_candidates: SponsorshipOperationalPseudonymSet,
    pub provider_request_candidates: SponsorshipOperationalPseudonymSet,
    pub launch_authority: SponsoredLaunchAuthority,
    pub acceptance: SponsoredGrantAcceptance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimGrantActivationCleanup {
    pub launch_id: LaunchId,
    pub holder_candidates: SponsorshipOperationalPseudonymSet,
    pub operation_client_request_candidates: SponsorshipOperationalPseudonymSet,
    pub operation: PendingProviderOperation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimInvalidatedGrantActivation {
    pub launch_id: LaunchId,
    pub holder_candidates: SponsorshipOperationalPseudonymSet,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginProviderGrantIssue {
    pub launch_id: LaunchId,
    pub expected_revision: u64,
    pub intent: GrantActivationIntent,
    pub holder: String,
    pub launch_authority: SponsoredLaunchAuthority,
    pub acceptance: SponsoredGrantAcceptance,
    /// Service time is metadata only. Persistence authorizes the transition
    /// and timestamps its durable state with authoritative database time.
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteGrantActivationCleanup {
    pub expected_revision: u64,
    pub holder: String,
    pub operation: PendingProviderOperation,
    /// The caller observed definitive retirement of the root credential. The
    /// store must independently re-prove that lineage in the completion CAS.
    pub global_credential_retirement: bool,
    pub intent: GrantActivationIntent,
    pub audit: SponsoredAuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteInvalidatedGrantActivation {
    pub expected_revision: u64,
    pub holder: String,
    pub intent: GrantActivationIntent,
    pub audit: SponsoredAuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitGrantActivation {
    pub expected_revision: u64,
    pub holder: String,
    pub intent: GrantActivationIntent,
    pub grant: SponsoredGrant,
    pub launch_authority: SponsoredLaunchAuthority,
    pub acceptance: SponsoredGrantAcceptance,
    pub gate_fence: SponsorshipGateFence,
    /// Service observation time for grant/audit metadata only. Persistence
    /// must use authoritative database time for policy/receipt expiry and the
    /// final compare-and-set decision.
    pub now: String,
    pub audit: SponsoredAuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsorshipPolicyCreationIdentity {
    pub request: SponsorshipOperationalPseudonymSet,
    pub command: SponsorshipOperationalPseudonymSet,
    /// Canonical digest of the caller payload before service-generated expiry
    /// timestamps are applied. It is non-reversible and lets persistence
    /// distinguish an exact replay from the same key carrying new contents.
    pub payload_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistSponsorshipPolicy {
    pub policy: SponsorshipPolicy,
    pub idempotency: SponsorshipPolicyCreationIdentity,
    pub gate_fence: SponsorshipGateFence,
    pub audit: SponsoredAuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistSponsoredAcceptance {
    pub acceptance: SponsoredGrantAcceptance,
    /// Complete bounded current/predecessor aliases for the raw idempotency
    /// request. Persistence may replace these aliases during replay, but must
    /// never rewrite the canonical identity embedded in `acceptance`.
    pub idempotency_candidates: SponsorshipOperationalPseudonymSet,
    pub launch_authority: SponsoredLaunchAuthority,
    pub gate_fence: SponsorshipGateFence,
    pub audit: SponsoredAuditEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantCapacityRelease {
    Expired,
    Exhausted,
    Revoked,
    Closed,
}

impl SponsoredGrant {
    pub fn calculate_capability_fingerprint(&self) -> Result<String, TriggerValidationError> {
        canonical_digest(&(
            "cma-inert-sponsored-capability-v1",
            (
                &self.id,
                self.generation,
                &self.tenant_id,
                &self.policy_id,
                self.policy_generation,
                self.credential_generation,
                &self.trigger_revision_id,
                &self.trigger_revision_digest,
                &self.launch_id,
            ),
            (
                &self.session_id,
                &self.contributor,
                &self.sponsor_subject,
                &self.terms_digest,
                &self.acceptance.digest,
                &self.authority.provider_grant_id_digest,
                self.authority.provider_state_version,
                &self.authority.expires_at,
                &self.authority.revocation_handle_digest,
                &self.allocation,
                &self.state,
            ),
        ))
    }

    pub fn refresh_capability(&mut self) -> Result<(), TriggerValidationError> {
        self.capability.grant_generation = self.generation;
        self.capability.policy_generation = self.policy_generation;
        self.capability.launch_id = self.launch_id.clone();
        self.capability.session_id = self.session_id.clone();
        self.capability.expires_at = self.authority.expires_at.clone();
        self.capability.fingerprint = self.calculate_capability_fingerprint()?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.schema_version != SPONSORSHIP_SCHEMA_VERSION
            || self.generation == 0
            || self.policy_generation == 0
            || self.credential_generation == 0
        {
            return Err(TriggerValidationError::field(
                "sponsored_grant",
                "has an unsupported schema or zero generation",
            ));
        }
        SponsoredGrantId::parse(self.id.0.clone())?;
        validate_identifier("sponsored_grant.tenant_id", &self.tenant_id, 255)?;
        validate_identifier(
            "sponsored_grant.sponsor_subject",
            &self.sponsor_subject.0,
            255,
        )?;
        validate_digest(
            "sponsored_grant.trigger_revision_digest",
            &self.trigger_revision_digest,
        )?;
        validate_digest("sponsored_grant.terms_digest", &self.terms_digest)?;
        SponsorshipOperationalPseudonym::parse(self.authority.provider_grant_id_digest.clone())?;
        SponsorshipOperationalPseudonym::parse(self.authority.revocation_handle_digest.clone())?;
        validate_timestamp("sponsored_grant.expires_at", &self.authority.expires_at)?;
        validate_timestamp("sponsored_grant.issued_at", &self.authority.issued_at)?;
        validate_timestamp("sponsored_grant.created_at", &self.created_at)?;
        validate_timestamp("sponsored_grant.updated_at", &self.updated_at)?;
        self.acceptance.validate()?;
        self.capability.validate()?;
        if !timestamp_before(&self.authority.issued_at, &self.authority.expires_at)
            || !timestamp_not_after(&self.acceptance.accepted_at, &self.authority.issued_at)
            || !timestamp_not_after(&self.acceptance.accepted_at, &self.created_at)
            || !timestamp_not_after(&self.authority.issued_at, &self.created_at)
            || !timestamp_before(&self.created_at, &self.authority.expires_at)
            || !timestamp_not_after(&self.created_at, &self.updated_at)
        {
            return Err(TriggerValidationError::field(
                "sponsored_grant.updated_at",
                "must issue only after acceptance, then create and update before expiry",
            ));
        }
        if self.launch_id != self.acceptance.launch_id
            || self.launch_id != self.capability.launch_id
            || self.session_id != self.capability.session_id
            || self.contributor != self.acceptance.contributor
            || self.policy_id != self.acceptance.policy_id
            || self.policy_generation != self.acceptance.policy_generation
            || self.policy_generation != self.capability.policy_generation
            || self.trigger_revision_id != self.acceptance.revision_id
            || self.trigger_revision_digest != self.acceptance.revision_digest
            || self.terms_digest != self.acceptance.terms_digest
            || self.authority.expires_at != self.capability.expires_at
            || self.generation != self.capability.grant_generation
            || self.capability.fingerprint != self.calculate_capability_fingerprint()?
        {
            return Err(TriggerValidationError::field(
                "sponsored_grant",
                "contains mismatched Launch, revision, policy, terms, session, or expiry bindings",
            ));
        }
        if !self.allocation.quota.is_nonzero()
            || self.allocation.spend_micros == 0
            || !self.usage.fits_within(&self.allocation)
            || self.authority.provider_state_version == 0
        {
            return Err(TriggerValidationError::field(
                "sponsored_grant.allocation",
                "must contain a positive provider state and usage within its reserved quota and budget",
            ));
        }
        if (self.state == SponsoredGrantState::RevocationPending)
            != self.pending_revocation.is_some()
        {
            return Err(TriggerValidationError::field(
                "sponsored_grant.pending_revocation",
                "must exist exactly while provider revocation is pending",
            ));
        }
        let definitively_revoked = matches!(
            self.state,
            SponsoredGrantState::Revoked | SponsoredGrantState::Closed
        );
        if definitively_revoked != self.revoked_at.is_some() {
            return Err(TriggerValidationError::field(
                "sponsored_grant.revoked_at",
                "must exist exactly for a revoked or closed grant",
            ));
        }
        if let Some(revoked_at) = &self.revoked_at {
            validate_timestamp("sponsored_grant.revoked_at", revoked_at)?;
            if !timestamp_not_after(&self.created_at, revoked_at)
                || !timestamp_not_after(&self.authority.issued_at, revoked_at)
                || !timestamp_not_after(revoked_at, &self.updated_at)
            {
                return Err(TriggerValidationError::field(
                    "sponsored_grant.revoked_at",
                    "must follow creation and issuance without exceeding the latest update",
                ));
            }
        }
        if let Some(operation) = &self.pending_revocation {
            operation.validate()?;
            if !operation_is_within_record(operation, &self.created_at, &self.updated_at) {
                return Err(TriggerValidationError::field(
                    "sponsored_grant.pending_revocation",
                    "must fall within the grant timestamp lineage",
                ));
            }
            if !matches!(
                operation.kind,
                ProviderOperationKind::RevokeGrant | ProviderOperationKind::CloseGrant
            ) {
                return Err(TriggerValidationError::field(
                    "sponsored_grant.pending_revocation.kind",
                    "must revoke or close this grant",
                ));
            }
        }
        Ok(())
    }

    pub fn is_active_at(&self, now: &str) -> bool {
        self.state == SponsoredGrantState::Active
            && crate::seconds_from_rfc3339(now).is_some_and(|now| {
                crate::seconds_from_rfc3339(&self.authority.expires_at)
                    .is_some_and(|expires| now < expires)
            })
    }
}

/// Secret-free contributor/sponsor projection of a grant. Provider request
/// identities, provider handles, revocation handles, and pending-operation
/// payloads never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredGrantView {
    pub id: SponsoredGrantId,
    pub policy_id: SponsorshipPolicyId,
    pub launch_id: LaunchId,
    pub session_id: SessionId,
    pub contributor: SubjectId,
    pub state: SponsoredGrantState,
    pub allocation: SponsoredUsageSummary,
    pub usage: SponsoredUsageSummary,
    pub capability: InertGrantCapability,
    pub expires_at: String,
    pub updated_at: String,
    pub provider_revocation_pending: bool,
}

impl From<&SponsoredGrant> for SponsoredGrantView {
    fn from(grant: &SponsoredGrant) -> Self {
        Self {
            id: grant.id.clone(),
            policy_id: grant.policy_id.clone(),
            launch_id: grant.launch_id.clone(),
            session_id: grant.session_id.clone(),
            contributor: grant.contributor.clone(),
            state: grant.state,
            allocation: grant.allocation.clone(),
            usage: grant.usage.clone(),
            capability: grant.capability.clone(),
            expires_at: grant.authority.expires_at.clone(),
            updated_at: grant.updated_at.clone(),
            provider_revocation_pending: grant.pending_revocation.is_some(),
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsoredUsageRequest {
    pub reservation_id: SponsoredUsageReservationId,
    /// Complete keyed aliases for caller-controlled request identity.
    pub request_identity: SponsorshipOperationalPseudonymSet,
    /// Digest of caller-controlled request payload only. Unlike
    /// `request_fingerprint`, this excludes the service clock and price quote
    /// so a stable retry can find the original reservation before repeating
    /// either observation.
    pub idempotency_fingerprint: String,
    pub grant_id: SponsoredGrantId,
    pub requester: SubjectId,
    pub tenant_id: String,
    pub trigger_revision_id: TriggerRevisionId,
    pub trigger_revision_digest: String,
    pub launch_id: LaunchId,
    pub session_id: SessionId,
    pub provider: String,
    pub model: String,
    pub resource: String,
    pub capability_fingerprint: String,
    pub maximum: SponsoredUsageSummary,
    pub price_quote_digest: String,
    pub requested_at: String,
}

impl SponsoredUsageRequest {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        SponsoredUsageReservationId::parse(self.reservation_id.0.clone())?;
        validate_digest(
            "usage.idempotency_fingerprint",
            &self.idempotency_fingerprint,
        )?;
        SponsoredGrantId::parse(self.grant_id.0.clone())?;
        TriggerRevisionId::parse(self.trigger_revision_id.0.clone())?;
        LaunchId::parse(self.launch_id.0.clone())?;
        validate_identifier("usage.tenant_id", &self.tenant_id, 255)?;
        validate_identifier("usage.requester", &self.requester.0, 255)?;
        validate_identifier("usage.session_id", &self.session_id.0, 255)?;
        validate_digest(
            "usage.trigger_revision_digest",
            &self.trigger_revision_digest,
        )?;
        validate_policy_token("usage.provider", &self.provider)?;
        validate_policy_token("usage.model", &self.model)?;
        validate_policy_token("usage.resource", &self.resource)?;
        validate_digest("usage.capability_fingerprint", &self.capability_fingerprint)?;
        validate_digest("usage.price_quote_digest", &self.price_quote_digest)?;
        validate_timestamp("usage.requested_at", &self.requested_at)?;
        if !self.maximum.quota.is_nonzero() || self.maximum.spend_micros == 0 {
            return Err(TriggerValidationError::field(
                "usage.maximum",
                "must reserve positive request, token, and spend bounds",
            ));
        }
        Ok(())
    }

    pub fn fingerprint(&self) -> Result<String, TriggerValidationError> {
        self.validate()?;
        canonical_digest(&(
            "cma-sponsored-usage-request-v2",
            (
                &self.idempotency_fingerprint,
                &self.grant_id,
                &self.requester,
                &self.tenant_id,
                &self.trigger_revision_id,
                &self.trigger_revision_digest,
                &self.launch_id,
            ),
            (
                &self.session_id,
                &self.provider,
                &self.model,
                &self.resource,
                &self.capability_fingerprint,
                &self.maximum,
                &self.price_quote_digest,
                &self.requested_at,
            ),
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SponsoredUsageState {
    Reserved,
    Attempting,
    Ambiguous,
    Settled,
    Released,
}

/// Short durable ownership window for one provider dispatch attempt.
///
/// The store creates both timestamps from its authoritative clock. A retry may
/// observe the immutable provider operation while this lease is live, but it
/// cannot issue another provider call until the lease expires and a new claim
/// is atomically admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredUsageDispatchLease {
    pub holder: String,
    pub acquired_at: String,
    pub expires_at: String,
}

impl SponsoredUsageDispatchLease {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        let holder = Uuid::parse_str(&self.holder).map_err(|_| {
            TriggerValidationError::field("usage_dispatch_lease.holder", "must be a canonical UUID")
        })?;
        if holder.to_string() != self.holder {
            return Err(TriggerValidationError::field(
                "usage_dispatch_lease.holder",
                "must be a canonical UUID",
            ));
        }
        validate_timestamp("usage_dispatch_lease.acquired_at", &self.acquired_at)?;
        validate_timestamp("usage_dispatch_lease.expires_at", &self.expires_at)?;
        if !timestamp_not_after(&self.acquired_at, &self.expires_at)
            || self.acquired_at == self.expires_at
        {
            return Err(TriggerValidationError::field(
                "usage_dispatch_lease.expires_at",
                "must follow lease acquisition",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredUsageReservation {
    /// Opaque reconciliation handle. Caller request identities and keyed
    /// aliases are private persistence metadata and never enter this view.
    pub id: SponsoredUsageReservationId,
    pub idempotency_fingerprint: String,
    pub request_fingerprint: String,
    pub grant_id: SponsoredGrantId,
    pub price_quote: SponsoredPriceQuote,
    pub reserved: SponsoredUsageSummary,
    pub state: SponsoredUsageState,
    /// Monotonic provider-dispatch claim generation. Legacy and generic usage
    /// reservations remain at zero until they cross this model dispatch path.
    #[serde(default)]
    pub dispatch_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_lease: Option<SponsoredUsageDispatchLease>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actual: Option<SponsoredUsageSummary>,
}

impl SponsoredUsageReservation {
    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        SponsoredUsageReservationId::parse(self.id.0.clone())?;
        validate_digest(
            "usage_reservation.idempotency_fingerprint",
            &self.idempotency_fingerprint,
        )?;
        validate_digest(
            "usage_reservation.request_fingerprint",
            &self.request_fingerprint,
        )?;
        SponsoredGrantId::parse(self.grant_id.0.clone())?;
        self.price_quote.validate()?;
        validate_timestamp("usage_reservation.created_at", &self.created_at)?;
        validate_timestamp("usage_reservation.updated_at", &self.updated_at)?;
        if !timestamp_not_after(&self.created_at, &self.updated_at) {
            return Err(TriggerValidationError::field(
                "usage_reservation.updated_at",
                "must not precede creation",
            ));
        }
        if self.reserved != self.price_quote.maximum {
            return Err(TriggerValidationError::field(
                "usage_reservation.reserved",
                "must equal the durable trusted price quote maximum",
            ));
        }
        if let Some(lease) = &self.dispatch_lease {
            lease.validate()?;
            if !timestamp_not_after(&self.created_at, &lease.acquired_at)
                || !timestamp_not_after(&lease.acquired_at, &self.updated_at)
            {
                return Err(TriggerValidationError::field(
                    "usage_reservation.dispatch_lease",
                    "must belong to the reservation's current update",
                ));
            }
        }
        if self.state == SponsoredUsageState::Reserved
            && (self.dispatch_revision != 0 || self.dispatch_lease.is_some())
        {
            return Err(TriggerValidationError::field(
                "usage_reservation.dispatch",
                "a reserved operation must never have crossed provider dispatch",
            ));
        }
        if (self.state == SponsoredUsageState::Attempting)
            != (self.dispatch_revision > 0 && self.dispatch_lease.is_some())
        {
            return Err(TriggerValidationError::field(
                "usage_reservation.dispatch",
                "attempting state must carry one positive durable dispatch claim",
            ));
        }
        if (self.state == SponsoredUsageState::Settled) != self.actual.is_some() {
            return Err(TriggerValidationError::field(
                "usage_reservation.actual",
                "must exist exactly after confirmed settlement",
            ));
        }
        if self
            .actual
            .as_ref()
            .is_some_and(|actual| !actual.fits_within(&self.reserved))
        {
            return Err(TriggerValidationError::field(
                "usage_reservation.actual",
                "must fit within the reserved maximum",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReservationDisposition {
    Created,
    Existing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredUsageReservationOutcome {
    pub disposition: ReservationDisposition,
    pub reservation: SponsoredUsageReservation,
}

/// Exact canonical request body retained only for model-provider recovery.
///
/// The value deliberately has no serialization implementation and always
/// redacts diagnostics. A durable store must encrypt the exposed canonical
/// bytes and erase them atomically when the reservation becomes terminal.
pub struct SponsoredModelDispatchPayload {
    schema_version: u32,
    request_digest: String,
    canonical_request: Zeroizing<String>,
}

impl SponsoredModelDispatchPayload {
    pub fn from_request(
        request_digest: impl Into<String>,
        request: &Value,
    ) -> Result<Self, TriggerValidationError> {
        let request_digest = request_digest.into();
        let canonical_request =
            String::from_utf8(canonical_json_bytes(request)?).map_err(|_| {
                TriggerValidationError::field(
                    "sponsored_model.request",
                    "could not be represented as canonical UTF-8 JSON",
                )
            })?;
        let payload = Self {
            schema_version: SPONSORED_MODEL_DISPATCH_PAYLOAD_SCHEMA_VERSION,
            request_digest,
            canonical_request: Zeroizing::new(canonical_request),
        };
        payload.validate()?;
        Ok(payload)
    }

    pub fn from_encrypted_canonical_request(
        schema_version: u32,
        request_digest: impl Into<String>,
        canonical_request: impl Into<String>,
    ) -> Result<Self, TriggerValidationError> {
        let payload = Self {
            schema_version,
            request_digest: request_digest.into(),
            canonical_request: Zeroizing::new(canonical_request.into()),
        };
        payload.validate()?;
        Ok(payload)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }

    /// Exposed only to the encrypted store implementation. These bytes must
    /// never enter diagnostics, public serialization, or a runtime response.
    pub fn expose_canonical_request_for_encryption(&self) -> &str {
        self.canonical_request.as_str()
    }

    pub fn request(&self) -> Result<Value, TriggerValidationError> {
        self.validate()?;
        serde_json::from_str(self.canonical_request.as_str()).map_err(|_| {
            TriggerValidationError::field(
                "sponsored_model.request",
                "encrypted canonical JSON could not be decoded",
            )
        })
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.schema_version != SPONSORED_MODEL_DISPATCH_PAYLOAD_SCHEMA_VERSION {
            return Err(TriggerValidationError::field(
                "sponsored_model.dispatch_payload.schema_version",
                "is unsupported",
            ));
        }
        validate_digest(
            "sponsored_model.dispatch_payload.request_digest",
            &self.request_digest,
        )?;
        if self.canonical_request.is_empty()
            || self.canonical_request.len() > MAX_SPONSORED_MODEL_REQUEST_BYTES
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.dispatch_payload.request",
                "must fit within the sponsored model request bound",
            ));
        }
        let request: Value =
            serde_json::from_str(self.canonical_request.as_str()).map_err(|_| {
                TriggerValidationError::field(
                    "sponsored_model.dispatch_payload.request",
                    "must contain valid JSON",
                )
            })?;
        if !request.is_object()
            || canonical_json_bytes(&request)?.as_slice() != self.canonical_request.as_bytes()
            || lower_hex(Sha256::digest(self.canonical_request.as_bytes())) != self.request_digest
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.dispatch_payload.request",
                "must be canonical object JSON matching its digest",
            ));
        }
        Ok(())
    }
}

impl Clone for SponsoredModelDispatchPayload {
    fn clone(&self) -> Self {
        Self {
            schema_version: self.schema_version,
            request_digest: self.request_digest.clone(),
            canonical_request: Zeroizing::new(self.canonical_request.to_string()),
        }
    }
}

impl PartialEq for SponsoredModelDispatchPayload {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.request_digest == other.request_digest
            && self.canonical_request.as_str() == other.canonical_request.as_str()
    }
}

impl Eq for SponsoredModelDispatchPayload {}

impl fmt::Debug for SponsoredModelDispatchPayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SponsoredModelDispatchPayload")
            .field("schema_version", &self.schema_version)
            .field("request_digest", &self.request_digest)
            .field("canonical_request", &"<redacted>")
            .finish()
    }
}

/// Exact canonical provider response retained only for settled replay.
///
/// The bytes have no serialization implementation, are always redacted from
/// diagnostics, and must be encrypted by persistence under the settled
/// reservation's authenticated accounting context.
pub struct SponsoredModelResponsePayload {
    schema_version: u32,
    request_digest: String,
    response_digest: String,
    actual_quota: SponsoredQuota,
    canonical_response: Zeroizing<String>,
}

impl SponsoredModelResponsePayload {
    pub fn from_response(
        request_digest: impl Into<String>,
        actual_quota: SponsoredQuota,
        response: &Value,
    ) -> Result<Self, TriggerValidationError> {
        let canonical_response =
            String::from_utf8(canonical_json_bytes(response)?).map_err(|_| {
                TriggerValidationError::field(
                    "sponsored_model.response",
                    "could not be represented as canonical UTF-8 JSON",
                )
            })?;
        let response_digest = lower_hex(Sha256::digest(canonical_response.as_bytes()));
        let payload = Self {
            schema_version: SPONSORED_MODEL_RESPONSE_PAYLOAD_SCHEMA_VERSION,
            request_digest: request_digest.into(),
            response_digest,
            actual_quota,
            canonical_response: Zeroizing::new(canonical_response),
        };
        payload.validate()?;
        Ok(payload)
    }

    pub fn from_encrypted_canonical_response(
        schema_version: u32,
        request_digest: impl Into<String>,
        response_digest: impl Into<String>,
        actual_quota: SponsoredQuota,
        canonical_response: impl Into<String>,
    ) -> Result<Self, TriggerValidationError> {
        let payload = Self {
            schema_version,
            request_digest: request_digest.into(),
            response_digest: response_digest.into(),
            actual_quota,
            canonical_response: Zeroizing::new(canonical_response.into()),
        };
        payload.validate()?;
        Ok(payload)
    }

    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }

    pub fn response_digest(&self) -> &str {
        &self.response_digest
    }

    pub fn actual_quota(&self) -> SponsoredQuota {
        self.actual_quota
    }

    pub fn expose_canonical_response_for_encryption(&self) -> &str {
        self.canonical_response.as_str()
    }

    pub fn response(&self) -> Result<Value, TriggerValidationError> {
        self.validate()?;
        serde_json::from_str(self.canonical_response.as_str()).map_err(|_| {
            TriggerValidationError::field(
                "sponsored_model.response",
                "encrypted canonical JSON could not be decoded",
            )
        })
    }

    pub fn validate(&self) -> Result<(), TriggerValidationError> {
        if self.schema_version != SPONSORED_MODEL_RESPONSE_PAYLOAD_SCHEMA_VERSION {
            return Err(TriggerValidationError::field(
                "sponsored_model.response_payload.schema_version",
                "is unsupported",
            ));
        }
        validate_digest(
            "sponsored_model.response_payload.request_digest",
            &self.request_digest,
        )?;
        validate_digest(
            "sponsored_model.response_payload.response_digest",
            &self.response_digest,
        )?;
        if self.actual_quota.requests != 1
            || self.canonical_response.is_empty()
            || self.canonical_response.len() > MAX_SPONSORED_MODEL_RESPONSE_BYTES
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.response_payload",
                "must contain one bounded provider response",
            ));
        }
        let response: Value =
            serde_json::from_str(self.canonical_response.as_str()).map_err(|_| {
                TriggerValidationError::field(
                    "sponsored_model.response_payload.response",
                    "must contain valid JSON",
                )
            })?;
        if !response.is_object()
            || canonical_json_bytes(&response)?.as_slice() != self.canonical_response.as_bytes()
            || lower_hex(Sha256::digest(self.canonical_response.as_bytes())) != self.response_digest
        {
            return Err(TriggerValidationError::field(
                "sponsored_model.response_payload.response",
                "must be canonical object JSON matching its digest",
            ));
        }
        Ok(())
    }
}

impl Clone for SponsoredModelResponsePayload {
    fn clone(&self) -> Self {
        Self {
            schema_version: self.schema_version,
            request_digest: self.request_digest.clone(),
            response_digest: self.response_digest.clone(),
            actual_quota: self.actual_quota,
            canonical_response: Zeroizing::new(self.canonical_response.to_string()),
        }
    }
}

impl PartialEq for SponsoredModelResponsePayload {
    fn eq(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.request_digest == other.request_digest
            && self.response_digest == other.response_digest
            && self.actual_quota == other.actual_quota
            && self.canonical_response.as_str() == other.canonical_response.as_str()
    }
}

impl Eq for SponsoredModelResponsePayload {}

impl fmt::Debug for SponsoredModelResponsePayload {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SponsoredModelResponsePayload")
            .field("schema_version", &self.schema_version)
            .field("request_digest", &self.request_digest)
            .field("response_digest", &self.response_digest)
            .field("actual_quota", &self.actual_quota)
            .field("canonical_response", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SponsoredModelRecoveryRecord {
    pub reservation: SponsoredUsageReservation,
    pub dispatch: SponsoredModelDispatchPayload,
}

impl fmt::Debug for SponsoredModelRecoveryRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SponsoredModelRecoveryRecord")
            .field("reservation", &self.reservation)
            .field("dispatch", &self.dispatch)
            .finish()
    }
}

pub(super) fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, TriggerValidationError> {
    serde_json::to_vec(&canonical_json(value)).map_err(|_| {
        TriggerValidationError::field("sponsored_model.request", "could not be canonicalized")
    })
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&object[key]));
            }
            Value::Object(canonical)
        }
        scalar => scalar.clone(),
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ReserveSponsoredUsage {
    pub request: SponsoredUsageRequest,
    pub price_quote: SponsoredPriceQuote,
    pub launch_authority: SponsoredLaunchAuthority,
    pub gate_fence: SponsorshipGateFence,
    pub policy_generation: u64,
    /// Must match the grant's issuing credential generation. Persistence must
    /// not compare it to the current root generation after a successful
    /// rotation; current credential state remains the revocation fence.
    pub credential_generation: u64,
    /// Present only for the model dispatch path. Generic accounting
    /// reservations carry no provider payload.
    pub model_dispatch: Option<SponsoredModelDispatchPayload>,
    /// Service request time retained in reservation/audit metadata. It is not
    /// authoritative for expiry or rate limiting; those checks use database
    /// time in the reservation transaction.
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimSponsoredUsageDispatch {
    pub reservation_id: SponsoredUsageReservationId,
    pub request_fingerprint: String,
    pub expected_state: SponsoredUsageState,
    pub expected_dispatch_revision: u64,
    pub capability: InertGrantCapability,
    pub gate_fence: SponsorshipGateFence,
    /// Opaque claim identity. Persistence supplies all lease timestamps.
    pub holder: String,
    pub audit: SponsoredAuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SponsoredUsageDispatchClaim {
    pub acquired: bool,
    pub reservation: SponsoredUsageReservation,
}

/// Reconciliation authority for mutations derived from provider dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SponsoredUsageReconciliationFence {
    /// Completion emitted by the exact durable claim owner.
    DispatchClaim { revision: u64, holder: String },
    /// Definitive provider observation for this exact durable generation.
    ProviderObservation {
        state: SponsoredUsageState,
        revision: u64,
    },
    /// First dispatch was definitively refused before any provider attempt.
    NeverDispatched,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcileSponsoredUsage {
    pub reservation_id: SponsoredUsageReservationId,
    pub request_fingerprint: String,
    pub actual_quota: Option<SponsoredQuota>,
    /// Filled by `SponsorshipService` from the trusted pricing port. Callers
    /// cannot choose this value.
    pub actual: Option<SponsoredUsageSummary>,
    pub outcome: SponsoredUsageState,
    pub observed_at: String,
    pub dispatch_fence: Option<SponsoredUsageReconciliationFence>,
    /// Present only when a sponsored model settlement atomically exchanges
    /// request custody for exact encrypted response replay.
    pub model_response: Option<SponsoredModelResponsePayload>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SponsoredAuditAction {
    CredentialRegistered,
    CredentialRotated,
    CredentialRevocationRequested,
    CredentialRevoked,
    PolicyCreated,
    PolicySuspended,
    PolicyRevoked,
    TermsAccepted,
    GrantIssued,
    GrantDenied,
    UncommittedGrantRevoked,
    GrantRevocationRequested,
    GrantRevoked,
    GrantClosed,
    GrantExpired,
    UsageReserved,
    UsageDispatchClaimed,
    UsageDenied,
    UsageReconciled,
    AbuseReported,
    KillSwitchEngaged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SponsoredAuditTarget {
    Platform,
    Policy(SponsorshipPolicyId),
    Grant {
        policy_id: SponsorshipPolicyId,
        grant_id: SponsoredGrantId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsoredAuditEvent {
    pub event_id: String,
    pub action: SponsoredAuditAction,
    pub actor_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_id: Option<SponsorshipPolicyId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<SponsoredGrantId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_request_id_digest: Option<String>,
    pub outcome_code: String,
    pub occurred_at: String,
}

impl SponsoredAuditEvent {
    pub fn privacy_safe(
        pseudonymizer: &SponsorshipAuditPseudonymizer,
        action: SponsoredAuditAction,
        actor: &SubjectId,
        target: SponsoredAuditTarget,
        stable_request_id: Option<&str>,
        outcome_code: impl Into<String>,
        occurred_at: impl Into<String>,
    ) -> Self {
        let (policy_id, grant_id) = match target {
            SponsoredAuditTarget::Platform => (None, None),
            SponsoredAuditTarget::Policy(policy_id) => (Some(policy_id), None),
            SponsoredAuditTarget::Grant {
                policy_id,
                grant_id,
            } => (Some(policy_id), Some(grant_id)),
        };
        Self {
            event_id: Uuid::new_v4().to_string(),
            action,
            actor_digest: pseudonymizer.actor_digest(actor),
            policy_id,
            grant_id,
            stable_request_id_digest: stable_request_id
                .map(|value| pseudonymizer.request_digest(value)),
            outcome_code: outcome_code.into(),
            occurred_at: occurred_at.into(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SponsorshipPolicyUsage {
    pub consumed: SponsoredUsageSummary,
    /// Aggregate allowance still committed to active or ambiguously revoked
    /// grants. Settlement moves value from `reserved` to `consumed`; expiry or
    /// confirmed closure releases only the unused remainder.
    pub reserved: SponsoredUsageSummary,
    pub active_launches: u32,
}

impl SponsorshipPolicyUsage {
    pub fn exposure(&self) -> Result<SponsoredUsageSummary, TriggerValidationError> {
        self.consumed.checked_add(&self.reserved).ok_or_else(|| {
            TriggerValidationError::field("sponsorship_policy.usage", "exceeds numeric bounds")
        })
    }
}

#[cfg(test)]
mod operational_pseudonym_tests {
    use super::*;

    fn pseudonymizer() -> SponsorshipOperationalPseudonymizer {
        SponsorshipOperationalPseudonymizer::new(
            "current.2026:08",
            vec![0x11; 32],
            vec![("previous.2026:07".to_string(), vec![0x22; 32])],
        )
        .unwrap()
    }

    #[test]
    fn pseudonym_encoding_uses_the_final_separator_and_rejects_alternate_spelling() {
        let generated = pseudonymizer()
            .pseudonymize(
                SponsorshipOperationalDomain::UsageRequest,
                &["grant-a", "requester-a", "predictable"],
            )
            .unwrap();
        assert_eq!(generated.current().key_id(), "current.2026:08");
        assert_eq!(
            SponsorshipOperationalPseudonym::parse(generated.current().as_str())
                .unwrap()
                .as_str(),
            generated.current().as_str()
        );

        let uppercase = generated.current().as_str().to_ascii_uppercase();
        assert!(SponsorshipOperationalPseudonym::parse(uppercase).is_err());
        assert!(SponsorshipOperationalPseudonym::parse(format!(
            "sp1.current.2026:08.{}",
            "0".repeat(63)
        ))
        .is_err());
    }

    #[test]
    fn configured_key_validation_and_rotation_candidates_are_bounded() {
        let old = SponsorshipOperationalPseudonymizer::new(
            "previous.2026:07",
            vec![0x22; 32],
            Vec::new(),
        )
        .unwrap()
        .pseudonymize(
            SponsorshipOperationalDomain::CredentialRegistrationRequest,
            &["owner-a", "request-a"],
        )
        .unwrap()
        .current()
        .clone();
        let rotated = pseudonymizer()
            .pseudonymize(
                SponsorshipOperationalDomain::CredentialRegistrationRequest,
                &["owner-a", "request-a"],
            )
            .unwrap();
        assert!(rotated.matches(&old));
        assert_eq!(rotated.iter().count(), 2);

        let unknown =
            SponsorshipOperationalPseudonym::parse(format!("sp1.retired-key.{}", "a".repeat(64)))
                .unwrap();
        assert!(pseudonymizer().require_configured(&unknown).is_err());
    }

    #[test]
    fn domain_and_length_framing_prevent_context_collisions() {
        let keys = pseudonymizer();
        let split = keys
            .pseudonymize(
                SponsorshipOperationalDomain::CredentialRegistrationCommand,
                &["ab", "c"],
            )
            .unwrap();
        let joined = keys
            .pseudonymize(
                SponsorshipOperationalDomain::CredentialRegistrationCommand,
                &["a", "bc"],
            )
            .unwrap();
        let other_domain = keys
            .pseudonymize(SponsorshipOperationalDomain::UsageRequest, &["ab", "c"])
            .unwrap();
        assert_ne!(split.current(), joined.current());
        assert_ne!(split.current(), other_domain.current());
    }

    #[test]
    fn rejected_key_inputs_and_unbounded_contexts_take_guarded_drop_paths() {
        assert!(SponsorshipOperationalPseudonymizer::new(
            "invalid/key",
            vec![0x31; 32],
            Vec::new(),
        )
        .is_err());
        assert!(SponsorshipOperationalPseudonymizer::new(
            "current",
            vec![0x31; 32],
            vec![
                ("previous-a".into(), vec![0x41; 32]),
                ("invalid/late".into(), vec![0x42; 32]),
            ],
        )
        .is_err());
        assert!(SponsorshipOperationalPseudonymizer::new(
            "current",
            vec![0x31; 32],
            (0..=MAX_PREVIOUS_SPONSORSHIP_OPERATIONAL_KEYS)
                .map(|index| (format!("previous-{index}"), vec![index as u8; 32])),
        )
        .is_err());

        let keys = pseudonymizer();
        let too_many = vec![""; MAX_SPONSORSHIP_OPERATIONAL_CONTEXT_COMPONENTS + 1];
        assert!(keys
            .pseudonymize(SponsorshipOperationalDomain::UsageRequest, &too_many)
            .is_err());
        let too_large = "x".repeat(64 * 1024 + 1);
        assert!(keys
            .pseudonymize(
                SponsorshipOperationalDomain::CredentialReplacement,
                &[&too_large],
            )
            .is_err());
    }
}
