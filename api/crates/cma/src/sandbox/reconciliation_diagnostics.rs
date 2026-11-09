//! Store-specific outcomes without exposing identities or error text.
use super::*;
use layer3_adapter::binding_diagnostics::Outcome;

pub(super) fn store_outcome<T>(result: &Result<T, SandboxStoreError>) -> Outcome {
    match result {
        Ok(_) => Outcome::Complete,
        Err(SandboxStoreError::Unavailable(_)) => Outcome::Unavailable,
        Err(SandboxStoreError::Corrupt(_)) => Outcome::Corrupt,
        Err(SandboxStoreError::Conflict) => Outcome::Conflict,
        Err(_) => Outcome::Refused,
    }
}

pub(super) fn loss_outcome(
    result: &Result<SandboxRuntimeLossOutcome, SandboxStoreError>,
) -> Outcome {
    match result {
        Ok(SandboxRuntimeLossOutcome::RecoveryScheduled) => Outcome::RecoveryScheduled,
        Ok(SandboxRuntimeLossOutcome::Failed) => Outcome::Failed,
        Ok(SandboxRuntimeLossOutcome::Superseded) => Outcome::Superseded,
        Ok(SandboxRuntimeLossOutcome::NotEligible) => Outcome::NotEligible,
        Err(_) => store_outcome(result),
    }
}
