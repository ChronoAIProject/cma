//! Bounded persistence effects for the account record erasure step.
#![cfg_attr(
    not(test),
    allow(
        dead_code,
        unused_imports,
        reason = "record effects are consumed by the separately delivered account erasure worker"
    )
)]
pub(crate) mod catalog;
