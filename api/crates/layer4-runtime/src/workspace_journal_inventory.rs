//! Pure validation for offline inventory; never reconcile or recover a record.
use std::path::Path;

use super::{corrupt, io, validate_stored, Record, MAX_JOURNAL_BYTES};
use crate::config::RuntimeConfig;
use crate::failure::Result;

pub(crate) fn validate_inventory(
    bytes: &[u8],
    config: &RuntimeConfig,
    path: &Path,
    temporary: bool,
) -> Result<()> {
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(corrupt("workspace operation journal entry is oversized"));
    }
    let record: Record = serde_json::from_slice(bytes)
        .map_err(|_| corrupt("workspace operation journal payload is invalid"))?;
    if record.sandbox_id != config.sandbox_id
        || (!temporary && path != io::journal_path(&config.state_root, &record.operation_id))
    {
        return Err(corrupt("workspace operation journal identity is invalid"));
    }
    validate_stored(&record, &config.home, config.allow_local_git)
}
