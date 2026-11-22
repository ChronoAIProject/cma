use super::worker::*;
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Write};

fn field(value: &str, max: usize) -> Result<(), JournalError> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(JournalError::Capacity);
    }
    Ok(())
}
fn lease(seconds: u64) -> Result<(), JournalError> {
    if !(1..=MAX_LEASE_SECONDS).contains(&seconds) {
        return Err(JournalError::Invalid);
    }
    Ok(())
}
pub(super) fn key(
    binding: &JournalBinding,
    command: &JournalCommand,
) -> Result<String, JournalError> {
    if binding.contract.version != JOURNAL_VERSION
        || !crate::document_artifacts::valid_id(binding.deletion_id.as_str(), "acd_")
        || binding.sequence >= i64::MAX as u64
        || binding.inventory_generation >= i64::MAX as u64
    {
        return Err(JournalError::Invalid);
    }
    match command {
        JournalCommand::Enroll if binding.sequence != 0 || binding.inventory_generation != 0 => {
            return Err(JournalError::Invalid)
        }
        JournalCommand::Enroll => {}
        JournalCommand::Claim {
            request_id,
            holder,
            lease_seconds,
        } => {
            field(request_id, 128)?;
            field(holder, 128)?;
            lease(*lease_seconds)?;
        }
        JournalCommand::Prepare { page } => {
            if page.targets.len() > MAX_PLAN_TARGETS {
                return Err(JournalError::Capacity);
            }
            let mut seen = BTreeSet::new();
            for target in &page.targets {
                field(&target.namespace, 64)?;
                field(&target.identity, 512)?;
                field(&target.incarnation, 512)?;
                if !seen.insert(target) {
                    return Err(JournalError::Invalid);
                }
            }
            if let Some(cursor) = &page.cursor {
                field(cursor, 4096)?;
            }
        }
        JournalCommand::Observe { plan, .. } => {
            if !(plan.len() == 68
                && plan.starts_with("aeo_")
                && plan[4..]
                    .bytes()
                    .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase()))
            {
                return Err(JournalError::Invalid);
            }
        }
        JournalCommand::Renew { lease_seconds } => lease(*lease_seconds)?,
        JournalCommand::Defer { delay_seconds } => {
            if !(1..=86400).contains(delay_seconds) {
                return Err(JournalError::Invalid);
            }
        }
        JournalCommand::Block { .. } => {}
    }
    let mut digest = Sha256::new();
    let domain = b"cma.account-erasure.operation.v1";
    digest.update((domain.len() as u64).to_be_bytes());
    digest.update(domain);
    for bytes in [bounded(binding)?, bounded(command)?] {
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    Ok(format!("aeo_{:x}", digest.finalize()))
}

fn bounded(value: &impl serde::Serialize) -> Result<Vec<u8>, JournalError> {
    struct Output(Vec<u8>);
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > MAX_JOURNAL_BYTES {
                return Err(std::io::Error::other("journal capacity"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut out = Output(Vec::new());
    serde_json::to_writer(&mut out, value).map_err(|_| JournalError::Capacity)?;
    Ok(out.0)
}
