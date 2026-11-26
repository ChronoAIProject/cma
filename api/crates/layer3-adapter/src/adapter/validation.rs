use crate::{AdapterError, Result};

pub(super) fn require_admitted_context(operation_sequence: u64) -> Result<()> {
    if operation_sequence == 0 {
        return Err(unsupported(
            "context recovery requires an admitted operation",
        ));
    }
    Ok(())
}

pub(super) fn unsupported(detail: &str) -> AdapterError {
    AdapterError::Unsupported(detail.into())
}
