use async_trait::async_trait;
use serde::Serialize;

use crate::profiles::{content_digest, SandboxProfileSnapshot};

use super::SandboxStoreError;
use crate::sandbox::{Sandbox, SandboxLimits, SandboxOrigin, SandboxSignInState, SandboxStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", content = "value", rename_all = "snake_case")]
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
            || self.sandbox.status != SandboxStatus::Provisioning
            || self.sandbox.name != self.name.value()
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "sandbox admission requires a fresh aggregate and matching requested name",
            ));
        }
        if let SandboxOrigin::Trigger { launch_id } = &self.sandbox.origin {
            if launch_id.trim().is_empty() || launch_id.len() > 128 {
                return Err(SandboxStoreError::InvalidTransaction(
                    "sandbox trigger origin is invalid",
                ));
            }
        }
        if !valid_admission_fingerprint(&self.admission_fingerprint)
            || self.admission_fingerprint != fingerprint(&self.sandbox, &self.name)?
        {
            return Err(SandboxStoreError::InvalidTransaction(
                "sandbox admission fingerprint is invalid",
            ));
        }
        Ok(())
    }

    pub fn trigger_launch_id(&self) -> Option<&str> {
        match &self.sandbox.origin {
            SandboxOrigin::Manual => None,
            SandboxOrigin::Trigger { launch_id } => Some(launch_id),
        }
    }
}

pub fn valid_admission_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
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
        sign_in: sandbox.credential.sign_in,
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
    sign_in: SandboxSignInState,
    limits: SandboxLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxAdmissionResult {
    Created(Sandbox),
    Existing(Sandbox),
}

/// Atomic origin replay, live-limit admission, name allocation, insert and Created event.
#[async_trait]
pub trait SandboxAdmissionStore: Send + Sync {
    async fn admit_sandbox(
        &self,
        admission: SandboxAdmission,
    ) -> Result<SandboxAdmissionResult, SandboxStoreError>;
}
