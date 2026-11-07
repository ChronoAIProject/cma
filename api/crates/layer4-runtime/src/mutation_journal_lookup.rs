//! Keyed replay after the supervisor's full admission/recovery validation.
//! This removes a redundant record scan, not the bounded global integrity gates.
use super::{corrupt, ensure_exact, io, records, validation, Operation, Record};
use crate::config::RuntimeConfig;
use crate::failure::Result;

pub(super) fn find(config: &RuntimeConfig, operation: &Operation) -> Result<Option<Record>> {
    let mut count = 0;
    for root in crate::mutation_scope::roots(config)? {
        // V1 permits replacement names without the operation key in their name.
        // Any replacement therefore needs the original full reconciliation;
        // returning early could leave the one physical write slot occupied.
        if io::has_replacements(&root, &mut count)? {
            return records(config).map(|records| {
                records
                    .into_iter()
                    .find(|record| record.operation_id == operation.operation_id)
            });
        }
    }
    let path = io::journal_path(
        &crate::mutation_scope::root(config, &operation.subject),
        &operation.operation_id,
    );
    let Some(record) = io::read_record(&path)? else {
        return Ok(None);
    };
    validation::validate_record(&record, config)?;
    if record.sandbox_id != config.sandbox_id {
        return Err(corrupt(
            "runtime mutation journal belongs to another sandbox",
        ));
    }
    ensure_exact(&record, operation)?;
    Ok(Some(record))
}
