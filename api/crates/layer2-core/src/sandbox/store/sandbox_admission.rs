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

#[derive(Debug, Clone)]
pub struct SandboxAdmission {
    pub binding: Option<crate::sandbox::binding::BindingAdmissionFence>,
    pub sandbox: Sandbox,
    pub name: SandboxAdmissionName,
    pub admission_fingerprint: String,
    pub live_limit: u16,
    pub event_at: String,
}

impl SandboxAdmission {
    pub fn new(
        sandbox: Sandbox,
        name: SandboxAdmissionName,
        live_limit: u16,
        event_at: String,
    ) -> Result<Self, SandboxStoreError> {
        let admission_fingerprint = fingerprint(&sandbox, &name)?;
        let admission = Self {
            binding: None,
            sandbox,
            name,
            admission_fingerprint,
            live_limit,
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
}

impl SandboxAdmissionReplay {
    pub fn new(
        sandbox: Sandbox,
        name: SandboxAdmissionName,
        admission_fingerprint: String,
    ) -> Result<Self, SandboxStoreError> {
        let replay = Self {
            sandbox,
            name,
            admission_fingerprint,
        };
        replay.validate()?;
        Ok(replay)
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
            || self.admission_fingerprint != fingerprint(&self.sandbox, &self.name)?
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
    let mut reaches = sandbox.credential.reaches.clone();
    reaches.sort();
    reaches.dedup();
    content_digest(&AdmissionIdentity {
        owner: &sandbox.owner,
        origin: &sandbox.origin,
        name,
        profile: &sandbox.profile,
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
    #[serde(skip_serializing_if = "crate::sandbox::SandboxCredentialSource::is_managed")]
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
}
