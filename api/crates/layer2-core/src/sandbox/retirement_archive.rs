//! A permanent archive requirement survives flag changes and worker restarts.
//! Only the archive coordinator may add or complete it; ordinary saves preserve it.
use serde::{Deserialize, Serialize};

use super::{Sandbox, SandboxModelError, SandboxStatus};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementArchiveReceipt {
    pub manifest_sha256: String,
    pub custody_sha256: String,
    pub verification_sha256: String,
}

impl RetirementArchiveReceipt {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        if [
            &self.manifest_sha256,
            &self.custody_sha256,
            &self.verification_sha256,
        ]
        .into_iter()
        .all(|v| digest(v))
        {
            Ok(())
        } else {
            Err(invalid())
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetirementArchiveRequirement {
    pub format: u32,
    pub operation_id: String,
    pub plan_sha256: String,
    pub source_sha256: String,
    pub exclusion_sha256: String,
    /// None is a durable non-destructive hold, never an expired lease.
    pub verified: Option<RetirementArchiveReceipt>,
}

impl RetirementArchiveRequirement {
    pub fn validate(&self) -> Result<(), SandboxModelError> {
        let operation = uuid::Uuid::parse_str(&self.operation_id).map_err(|_| invalid())?;
        if self.format != 1
            || operation.is_nil()
            || operation.to_string() != self.operation_id
            || ![
                &self.plan_sha256,
                &self.source_sha256,
                &self.exclusion_sha256,
            ]
            .into_iter()
            .all(|v| digest(v))
        {
            return Err(invalid());
        }
        if let Some(receipt) = &self.verified {
            receipt.validate()?;
        }
        Ok(())
    }
}

impl Sandbox {
    pub fn retirement_archive_pending(&self) -> bool {
        self.lifecycle
            .retirement_archive
            .as_ref()
            .is_some_and(|gate| gate.verified.is_none())
    }

    pub fn validate_retirement_archive(&self) -> Result<(), SandboxModelError> {
        if let Some(gate) = &self.lifecycle.retirement_archive {
            gate.validate()?;
            if !matches!(
                self.status,
                SandboxStatus::Retiring | SandboxStatus::Retired
            ) || (self.status == SandboxStatus::Retired && gate.verified.is_none())
            {
                return Err(invalid());
            }
        }
        Ok(())
    }

    /// The supported coordinator writes its two transitions directly with a
    /// revision CAS and immutable evidence. Other persistence cannot clear,
    /// manufacture or replace a requirement/receipt, even when disabled.
    pub fn validate_retirement_archive_successor(
        &self,
        next: &Self,
    ) -> Result<(), SandboxModelError> {
        if self.lifecycle.retirement_archive != next.lifecycle.retirement_archive
            || self.retirement_archive_pending()
        {
            return Err(invalid());
        }
        next.validate_retirement_archive()
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}

fn invalid() -> SandboxModelError {
    SandboxModelError::invalid_value("permanent retirement archive requirement is invalid or held")
}

#[cfg(test)]
#[path = "retirement_archive_tests.rs"]
mod tests;
