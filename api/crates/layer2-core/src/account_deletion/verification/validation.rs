use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CoverageError {
    #[error("unsupported erasure coverage contract")]
    UnsupportedVersion,
    #[error("invalid erasure verification binding")]
    InvalidBinding,
    #[error("erasure observations belong to a different sealed plan")]
    BindingChanged,
    #[error("erasure coverage exceeds its bounded input contract")]
    CapacityExceeded,
    #[error("invalid erasure catalogue scope")]
    InvalidScope,
    #[error("erasure catalogue omits required class {0:?}")]
    MissingClass(ResidueClass),
    #[error("duplicate erasure coverage scope")]
    DuplicateScope,
    #[error("erasure observations include an unclassified scope")]
    UnclassifiedScope,
    #[error("erasure observations omit a required scope")]
    MissingScope,
    #[error("erasure observation is incomplete")]
    Incomplete,
    #[error("erasure residue remains")]
    ResidueFound,
    #[error("erasure observation does not match the approved policy")]
    PolicyMismatch,
    #[error("erasure retention approval has expired")]
    RetentionExpired,
    #[error("erasure evidence has an invalid observation time")]
    InvalidTime,
    #[error("erasure scan is incomplete or repeats a cursor")]
    IncompleteScan,
}

/// Checked structure only. This value cannot authorize deletion completion.
/// The production proof store must authenticate independent evidence and consume
/// it atomically against the current sealed plan before completing any request.
#[derive(Debug)]
pub struct ValidatedCoverage<'a> {
    plan: &'a CoveragePlan,
    observations: &'a CoverageObservations,
}

impl ValidatedCoverage<'_> {
    pub fn plan(&self) -> &CoveragePlan {
        self.plan
    }

    pub fn observations(&self) -> &CoverageObservations {
        self.observations
    }
}

impl CoveragePlan {
    pub fn validate<'a>(
        &'a self,
        observations: &'a CoverageObservations,
        checked_at: &str,
    ) -> Result<ValidatedCoverage<'a>, CoverageError> {
        if self.version != ERASURE_COVERAGE_VERSION
            || observations.version != ERASURE_COVERAGE_VERSION
        {
            return Err(CoverageError::UnsupportedVersion);
        }
        if self.binding != observations.binding {
            return Err(CoverageError::BindingChanged);
        }
        let now = timestamp(checked_at)?;
        let boundary = self.binding.observation_boundary(now)?;
        if self.requirements.len() > MAX_ERASURE_COVERAGE_SCOPES
            || observations.observations.len() > MAX_ERASURE_COVERAGE_SCOPES
        {
            return Err(CoverageError::CapacityExceeded);
        }
        let mut requirements = BTreeMap::new();
        let mut classes = BTreeSet::new();
        for requirement in &self.requirements {
            validate_scope(&requirement.scope)?;
            if requirements
                .insert(&requirement.scope, &requirement.disposition)
                .is_some()
            {
                return Err(CoverageError::DuplicateScope);
            }
            classes.insert(requirement.scope.class);
            if let RequiredDisposition::ApprovedRetained { approval } = &requirement.disposition {
                if timestamp(&approval.expires_at)? <= now {
                    return Err(CoverageError::RetentionExpired);
                }
            }
        }
        for class in REQUIRED_RESIDUE_CLASSES {
            if !classes.contains(&class) {
                return Err(CoverageError::MissingClass(class));
            }
        }
        let mut seen = BTreeSet::new();
        for entry in &observations.observations {
            if !seen.insert(&entry.scope) {
                return Err(CoverageError::DuplicateScope);
            }
            let requirement = requirements
                .get(&entry.scope)
                .ok_or(CoverageError::UnclassifiedScope)?;
            validate_observation(requirement, &entry.observation, boundary, now)?;
        }
        if seen.len() != requirements.len() {
            return Err(CoverageError::MissingScope);
        }
        Ok(ValidatedCoverage {
            plan: self,
            observations,
        })
    }
}

impl VerificationBinding {
    fn observation_boundary(&self, now: jiff::Timestamp) -> Result<jiff::Timestamp, CoverageError> {
        if self.deletion_id.as_str().is_empty()
            || self.deletion_id.as_str().len() > 128
            || !self
                .deletion_id
                .as_str()
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || self.checkpoint.inventory_generation == 0
        {
            return Err(CoverageError::InvalidBinding);
        }
        let requested = timestamp(&self.requested_at)?;
        let completed = timestamp(&self.checkpoint.effects_completed_at)?;
        let closed = timestamp(&self.checkpoint.writers_closed_at)?;
        if requested < jiff::Timestamp::UNIX_EPOCH
            || completed < requested
            || closed < requested
            || completed > now
            || closed > now
        {
            return Err(CoverageError::InvalidTime);
        }
        Ok(completed.max(closed))
    }
}

fn validate_scope(scope: &ResidueScope) -> Result<(), CoverageError> {
    if scope.namespace.is_empty()
        || scope.namespace.len() > 160
        || !scope.namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'.')
        })
    {
        return Err(CoverageError::InvalidScope);
    }
    Ok(())
}

fn validate_observation(
    required: &RequiredDisposition,
    observation: &ResidueObservation,
    boundary: jiff::Timestamp,
    now: jiff::Timestamp,
) -> Result<(), CoverageError> {
    match observation {
        ResidueObservation::Incomplete { .. } => Err(CoverageError::Incomplete),
        ResidueObservation::Residual { .. } => Err(CoverageError::ResidueFound),
        ResidueObservation::ObservedZero { scan } => {
            if scan.validate(boundary, now)? != 0 {
                return Err(CoverageError::ResidueFound);
            }
            if !matches!(required, RequiredDisposition::Erase) {
                return Err(CoverageError::PolicyMismatch);
            }
            Ok(())
        }
        ResidueObservation::ApprovedRetained { scan, approval } => {
            scan.validate(boundary, now)?;
            if !matches!(required, RequiredDisposition::ApprovedRetained { approval: expected } if expected == approval)
            {
                return Err(CoverageError::PolicyMismatch);
            }
            Ok(())
        }
        ResidueObservation::ProvenNotApplicable {
            observed_at,
            authority_evidence,
        } => {
            observation_time(observed_at, boundary, now)?;
            if !matches!(required, RequiredDisposition::ProvenNotApplicable { authority_evidence: expected } if expected == authority_evidence)
            {
                return Err(CoverageError::PolicyMismatch);
            }
            Ok(())
        }
    }
}

impl CompleteScan {
    fn validate(
        &self,
        boundary: jiff::Timestamp,
        now: jiff::Timestamp,
    ) -> Result<u64, CoverageError> {
        let started = observation_time(&self.started_at, boundary, now)?;
        observation_time(&self.finished_at, started, now)?;
        if self.pages.is_empty() {
            return Err(CoverageError::IncompleteScan);
        }
        if self.pages.len() > MAX_ERASURE_SCAN_PAGES {
            return Err(CoverageError::CapacityExceeded);
        }
        let mut cursor = None;
        let mut seen = BTreeSet::new();
        let mut remaining = 0_u64;
        for (index, page) in self.pages.iter().enumerate() {
            if page.cursor.as_ref() != cursor || !seen.insert(&page.cursor) {
                return Err(CoverageError::IncompleteScan);
            }
            if page.next_cursor.is_none() != (index + 1 == self.pages.len()) {
                return Err(CoverageError::IncompleteScan);
            }
            cursor = page.next_cursor.as_ref();
            remaining = remaining
                .checked_add(page.remaining)
                .ok_or(CoverageError::CapacityExceeded)?;
        }
        Ok(remaining)
    }
}

fn timestamp(value: &str) -> Result<jiff::Timestamp, CoverageError> {
    value.parse().map_err(|_| CoverageError::InvalidTime)
}

fn observation_time(
    value: &str,
    boundary: jiff::Timestamp,
    now: jiff::Timestamp,
) -> Result<jiff::Timestamp, CoverageError> {
    let at = timestamp(value)?;
    if at < boundary || at > now {
        return Err(CoverageError::InvalidTime);
    }
    Ok(at)
}
