//! Keyed replay after the supervisor's full admission/recovery validation.
//! This removes a redundant record scan, not the bounded global integrity gates.
use super::{corrupt, ensure_exact, io, records, validation, Operation, Record, Subject};
use crate::config::RuntimeConfig;
use crate::failure::Result;

pub(super) fn find(config: &RuntimeConfig, operation: &Operation) -> Result<Option<Record>> {
    read(
        config,
        &operation.operation_id,
        &operation.subject,
        Some(operation),
    )
}

pub(super) fn find_by_id(
    config: &RuntimeConfig,
    operation_id: &str,
    subject: &Subject,
) -> Result<Option<Record>> {
    read(config, operation_id, subject, None)
}

pub(super) fn find_for_proof(
    config: &RuntimeConfig,
    operation_id: &str,
    subject: &Subject,
) -> Result<Option<Record>> {
    let mut count = 0;
    for root in crate::mutation_scope::roots(config)? {
        if io::has_replacements(
            &root,
            &mut count,
            crate::mutation_recovery::journal_limit(config),
        )? {
            return Err(crate::failure::RuntimeError::unavailable(
                "agent_process_close_journal_unsettled",
                "Agent Close proof is unavailable while journal replacements are pending",
            ));
        }
    }
    read_keyed(config, operation_id, subject, None)
}

fn read(
    config: &RuntimeConfig,
    operation_id: &str,
    subject: &Subject,
    exact: Option<&Operation>,
) -> Result<Option<Record>> {
    let mut count = 0;
    for root in crate::mutation_scope::roots(config)? {
        // V1 permits replacement names without the operation key in their name.
        // Any replacement therefore needs the original full reconciliation;
        // returning early could leave the one physical write slot occupied.
        if io::has_replacements(
            &root,
            &mut count,
            crate::mutation_recovery::journal_limit(config),
        )? {
            let record = records(config)?
                .into_iter()
                .find(|record| record.operation_id == operation_id);
            if let (Some(record), Some(operation)) = (&record, exact) {
                ensure_exact(record, operation)?;
            }
            return Ok(record);
        }
    }
    read_keyed(config, operation_id, subject, exact)
}

fn read_keyed(
    config: &RuntimeConfig,
    operation_id: &str,
    subject: &Subject,
    exact: Option<&Operation>,
) -> Result<Option<Record>> {
    let path = io::journal_path(&crate::mutation_scope::root(config, subject), operation_id);
    let Some(record) = io::read_record(&path)? else {
        return Ok(None);
    };
    validation::validate_record(&record, config)?;
    if record.sandbox_id != config.sandbox_id {
        return Err(corrupt(
            "runtime mutation journal belongs to another sandbox",
        ));
    }
    if let Some(operation) = exact {
        ensure_exact(&record, operation)?;
    } else if record.operation_id != operation_id {
        return Err(corrupt(
            "runtime journal contains another operation identity",
        ));
    }
    io::sync_terminal(&path, &record)?;
    Ok(Some(record))
}
