use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ids::SubjectId;
use crate::profiles::{content_digest, SandboxProfileSnapshot};

use super::SandboxStoreError;
use crate::sandbox::{Sandbox, SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "mode",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum SandboxAdmissionName {
    Exact(String),
    Allocate(String),
}

impl SandboxAdmissionName {
    fn value(&self) -> &str {
        match self {
            Self::Exact(value) | Self::Allocate(value) => value,
        }
    }
}

/// The authority whose mutable Sandbox cap applies to this new admission.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SandboxLimitClass {
    #[default]
    Owner,
    Visitor,
}

impl SandboxLimitClass {
    pub fn current_limit(
        self,
        settings: &crate::PlatformSettings,
    ) -> Result<u16, SandboxStoreError> {
        let limit = match self {
            Self::Owner => settings.limits.max_sandboxes_per_user,
            Self::Visitor => settings.limits.visitor_max_sandboxes,
        };
        u16::try_from(limit)
            .map_err(|_| SandboxStoreError::Corrupt("Sandbox limit is out of range".into()))
    }
}

#[derive(Debug, Clone)]
pub struct SandboxAdmission {
    pub creation_fence: Option<CreationFence>,
    pub binding: Option<crate::sandbox::binding::BindingAdmissionFence>,
    lifecycle_lease: Option<LifecycleAdmissionLease>,
    pub sandbox: Sandbox,
    pub name: SandboxAdmissionName,
    pub admission_fingerprint: String,
    pub live_limit: u16,
    pub limit_class: SandboxLimitClass,
    pub event_at: String,
}

/// One accepted creation and, for lifecycle work, its current execution lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreationFence {
    pub record_id: String,
    pub generation: String,
    pub epoch: String,
    pub lifecycle_lease: Option<(String, String)>,
}

#[derive(Clone)]
struct LifecycleAdmissionLease(String);

impl std::fmt::Debug for LifecycleAdmissionLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LifecycleAdmissionLease(..)")
    }
}

impl SandboxAdmission {
    /// The initiating worker's in-memory claim. It is excluded from the
    /// replay fingerprint so a new worker can recover a committed admission.
    #[must_use]
    pub fn with_lifecycle_lease(mut self, token: &str) -> Self {
        self.lifecycle_lease = Some(LifecycleAdmissionLease(token.to_owned()));
        self
    }

    pub fn lifecycle_lease(&self) -> Option<&str> {
        self.lifecycle_lease.as_ref().map(|lease| lease.0.as_str())
    }

    pub fn new(
        sandbox: Sandbox,
        name: SandboxAdmissionName,
        live_limit: u16,
        event_at: String,
    ) -> Result<Self, SandboxStoreError> {
        let admission_fingerprint = fingerprint(&sandbox, &name)?;
        let admission = Self {
            creation_fence: None,
            binding: None,
            lifecycle_lease: None,
            sandbox,
            name,
            admission_fingerprint,
            live_limit,
            limit_class: SandboxLimitClass::Owner,
            event_at,
        };
        admission.validate()?;
        Ok(admission)
    }

    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.sandbox.validate().map_err(|error| {
            SandboxStoreError::Corrupt(format!("sandbox aggregate is invalid: {error}"))
        })?;
        if self.sandbox.revision != 1
            || !self.sandbox.credential.service_grants.is_empty()
            || self.sandbox.credential.service_scope.is_some()
            || self.sandbox.status != SandboxStatus::Provisioning
            || self.sandbox.name != self.name.value()
            || !valid_requested_name(self.name.value())
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "sandbox admission requires a fresh aggregate and matching requested name",
            ));
        }
        self.sandbox.origin.validate().map_err(|_| {
            SandboxStoreError::InvalidTransaction("sandbox admission origin is invalid")
        })?;
        if !valid_admission_fingerprint(&self.admission_fingerprint)
            || self.admission_fingerprint != fingerprint(&self.sandbox, &self.name)?
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "sandbox admission fingerprint is invalid",
            ));
        }
        Ok(())
    }

    pub fn origin_key(&self) -> Option<(&'static str, &str)> {
        self.sandbox.origin.replay_key()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxAdmissionReplay {
    pub sandbox: Sandbox,
    pub name: SandboxAdmissionName,
    pub admission_fingerprint: String,
    // Proven original policy is admission identity; sandbox.profile.always_on
    // remains the independently mutable current policy returned to callers.
    admitted_always_on: Option<bool>,
}

impl SandboxAdmissionReplay {
    pub fn new(
        sandbox: Sandbox,
        name: SandboxAdmissionName,
        admission_fingerprint: String,
    ) -> Result<Self, SandboxStoreError> {
        let replay = Self {
            admitted_always_on: sandbox.profile.always_on,
            sandbox,
            name,
            admission_fingerprint,
        };
        replay.validate()?;
        Ok(replay)
    }

    /// Read a retained receipt without replacing its original fingerprint.
    /// Historical rows stored the three-state policy only inside that digest.
    /// A complete digest match proves its original value; every other identity
    /// field must still match the current aggregate. Fresh admissions use `new`.
    pub fn from_retained(
        sandbox: Sandbox,
        name: SandboxAdmissionName,
        admission_fingerprint: String,
    ) -> Result<Self, SandboxStoreError> {
        if valid_admission_fingerprint(&admission_fingerprint) {
            for admitted_always_on in [None, Some(false), Some(true)] {
                if fingerprint_with_policy(&sandbox, &name, admitted_always_on)?
                    == admission_fingerprint
                {
                    let replay = Self {
                        sandbox,
                        name,
                        admission_fingerprint,
                        admitted_always_on,
                    };
                    replay.validate()?;
                    return Ok(replay);
                }
            }
        }
        Err(corrupt_replay(
            "profile snapshot or admission fingerprint is invalid",
        ))
    }

    pub fn validate(&self) -> Result<(), SandboxStoreError> {
        self.sandbox.validate().map_err(|error| {
            SandboxStoreError::Corrupt(format!("replayed sandbox aggregate is invalid: {error}"))
        })?;
        let Some((_, id)) = self.sandbox.origin.replay_key() else {
            return Err(corrupt_replay("origin has no replay identity"));
        };
        if id.trim().is_empty()
            || id.len() > 128
            || id.chars().any(char::is_control)
            || !valid_requested_name(self.name.value())
            || !stored_name_matches(&self.name, &self.sandbox.name)
        {
            return Err(corrupt_replay("origin or requested name is invalid"));
        }
        let profile_digest = content_digest(&self.sandbox.profile.content)
            .map_err(|error| corrupt_replay(&error.to_string()))?;
        if profile_digest != self.sandbox.profile.digest
            || !valid_admission_fingerprint(&self.admission_fingerprint)
            || self.admission_fingerprint
                != fingerprint_with_policy(&self.sandbox, &self.name, self.admitted_always_on)?
        {
            return Err(corrupt_replay(
                "profile snapshot or admission fingerprint is invalid",
            ));
        }
        Ok(())
    }
}

pub fn valid_admission_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

pub fn allocated_sandbox_name(base: &str, number: u64) -> Option<String> {
    if number == 0 || !valid_requested_name(base) {
        return None;
    }
    if number == 1 {
        return Some(base.to_string());
    }
    let suffix = format!("-{number}");
    let maximum = 64_usize.saturating_sub(suffix.len());
    let mut end = base.len().min(maximum);
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    Some(format!("{}{}", &base[..end], suffix))
}

fn stored_name_matches(requested: &SandboxAdmissionName, stored: &str) -> bool {
    match requested {
        SandboxAdmissionName::Exact(name) => name == stored,
        SandboxAdmissionName::Allocate(base) if base == stored => true,
        SandboxAdmissionName::Allocate(base) => stored
            .rsplit_once('-')
            .and_then(|(_, number)| number.parse::<u64>().ok())
            .filter(|number| *number >= 2)
            .and_then(|number| allocated_sandbox_name(base, number))
            .is_some_and(|expected| expected == stored),
    }
}

fn valid_requested_name(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 64 && !value.contains('\0')
}

fn corrupt_replay(detail: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(format!("sandbox admission replay {detail}"))
}

fn fingerprint(
    sandbox: &Sandbox,
    name: &SandboxAdmissionName,
) -> Result<String, SandboxStoreError> {
    fingerprint_with_policy(sandbox, name, sandbox.profile.always_on)
}

fn fingerprint_with_policy(
    sandbox: &Sandbox,
    name: &SandboxAdmissionName,
    admitted_always_on: Option<bool>,
) -> Result<String, SandboxStoreError> {
    let mut profile = sandbox.profile.clone();
    profile.always_on = admitted_always_on;
    let mut reaches = sandbox.credential.reaches.clone();
    reaches.sort();
    reaches.dedup();
    content_digest(&AdmissionIdentity {
        owner: &sandbox.owner,
        origin: &sandbox.origin,
        name,
        profile: &profile,
        reaches: &reaches,
        credential_source: &sandbox.credential.source,
        sign_in: AdmissionSignInMode::from(sandbox.credential.sign_in),
        limits: sandbox.limits,
    })
    .map_err(|error| SandboxStoreError::Corrupt(error.to_string()))
}

#[derive(Serialize)]
struct AdmissionIdentity<'a> {
    owner: &'a crate::ids::SubjectId,
    origin: &'a SandboxOrigin,
    name: &'a SandboxAdmissionName,
    profile: &'a SandboxProfileSnapshot,
    reaches: &'a [crate::ids::ServiceId],
    #[serde(skip_serializing_if = "crate::sandbox::SandboxCredentialSource::is_default_managed")]
    credential_source: &'a crate::sandbox::SandboxCredentialSource,
    sign_in: AdmissionSignInMode,
    limits: SandboxLimits,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum AdmissionSignInMode {
    NotRequired,
    OwnCredentials,
}

impl From<SandboxSignInState> for AdmissionSignInMode {
    fn from(state: SandboxSignInState) -> Self {
        match state {
            SandboxSignInState::NotRequired => Self::NotRequired,
            SandboxSignInState::Required | SandboxSignInState::Complete => Self::OwnCredentials,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxAdmissionResult {
    Created(Sandbox),
    Existing(Sandbox),
}

/// Atomic origin replay, live-limit admission, name allocation, insert and Created event.
#[async_trait]
pub trait SandboxAdmissionStore: Send + Sync {
    async fn sandbox_admission_replay(
        &self,
        owner: &SubjectId,
        origin: &SandboxOrigin,
    ) -> Result<Option<SandboxAdmissionReplay>, SandboxStoreError>;

    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError>;

    /// Catalogue-backed callers require exact pin admission with the new resource.
    async fn admit_catalogue_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError> {
        self.admit_sandbox(admission).await
    }
}

#[cfg(test)]
#[path = "sandbox_admission_policy_tests.rs"]
mod policy_tests;
