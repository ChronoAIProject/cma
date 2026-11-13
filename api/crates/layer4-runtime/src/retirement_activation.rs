//! In-process maintenance exclusion boundary. No constructor is exposed yet.
//!
//! Physical old-writer/provider exclusion is not qualified by the binding slice.
//! A future private constructor must validate authenticated durable C authority,
//! current binding, E0–E5 and real exclusion before constructing this value.
//! Neither serde, a boolean, nor an arbitrary caller-owned lock can produce it.

use layer3_adapter::runtime_ledger_maintenance::LedgerBindingV1;

/// Owned, non-Clone and non-serializable authority. Admitted detached storage work
/// must own this value (or its owning coordinator) until the work settles.
/// There is intentionally no public or test-only constructor granting authority.
pub struct ValidatedLedgerExclusion {
    binding: LedgerBindingV1,
    plan_sha256: String,
    backup_receipt_sha256: String,
    authority_receipt_sha256: String,
    _hold: Box<dyn Send>,
}

impl ValidatedLedgerExclusion {
    pub fn binding(&self) -> &LedgerBindingV1 {
        &self.binding
    }

    pub fn plan_sha256(&self) -> &str {
        &self.plan_sha256
    }

    pub fn backup_receipt_sha256(&self) -> &str {
        &self.backup_receipt_sha256
    }

    pub fn authority_receipt_sha256(&self) -> &str {
        &self.authority_receipt_sha256
    }
}

impl std::fmt::Debug for ValidatedLedgerExclusion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidatedLedgerExclusion")
            .field("binding", &self.binding)
            .finish_non_exhaustive()
    }
}
