use super::io::{Source, STATE, TRANSFER};
use super::{error, json, unsupported_version, Component, Status};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::install_state::InstallRecord;
use crate::persistent_home::Binding;

const BINDING_CHECKS: &str = "strict_binding_exact_source_identity_generation";
const INSTALL_CHECKS: &str = "strict_records_receipts_binding_join_unpublished_metadata";

pub(super) fn assess(
    config: &RuntimeConfig,
    source: &mut Source,
) -> Result<(Component, Component)> {
    let (binding, decoded) = binding(config, source)?;
    let installation = installation(config, source, decoded.as_ref())?;
    Ok((binding, installation))
}

fn binding(config: &RuntimeConfig, source: &mut Source) -> Result<(Component, Option<Binding>)> {
    let names = source.names(STATE, "home-binding")?.unwrap_or_default();
    let mut pending = false;
    let mut unknown = false;
    for name in &names {
        if name == "binding.json" {
            continue;
        }
        let id = name
            .strip_prefix(".home-")
            .and_then(|s| s.strip_suffix(".tmp"));
        if id.is_some_and(|id| id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())) {
            // Unpublished bytes are evidence, never cleanup candidates.
            source.record(STATE, &format!("home-binding/{name}"), 4096)?;
            pending = true;
        } else {
            unknown = true;
        }
    }
    let Some(bytes) = source.record(STATE, "home-binding/binding.json", 4096)? else {
        return Ok((
            Component::new(Status::Inconsistent, "binding_missing", BINDING_CHECKS),
            None,
        ));
    };
    if unsupported_version(&bytes, &[2])? {
        return Ok((
            Component::new(Status::Unsupported, "binding_version", BINDING_CHECKS),
            None,
        ));
    }
    let decoded = Binding::decode_inventory(&bytes, config).ok();
    let (status, reason) = if decoded.is_none() {
        (Status::Inconsistent, "binding_invalid")
    } else if unknown {
        (Status::Unsupported, "binding_entries")
    } else if pending {
        (Status::Inconsistent, "binding_publication_pending")
    } else {
        (Status::Recognized, "binding_matches")
    };
    Ok((Component::new(status, reason, BINDING_CHECKS), decoded))
}

fn installation(
    config: &RuntimeConfig,
    source: &mut Source,
    binding: Option<&Binding>,
) -> Result<Component> {
    let names = source.names(TRANSFER, "runtime-state")?.unwrap_or_default();
    let mut record = None;
    let mut pending = false;
    let mut invalid = false;
    let mut unsupported = false;
    let mut count = 0;
    for name in names {
        if name == crate::ledger_format::BARRIER || name == crate::retirement_format::BARRIER {
            continue;
        }
        count += 1;
        if count > 32 {
            return Err(error("entry_budget"));
        }
        let temporary = match crate::install_state::inspection_temporary_name(&name) {
            Ok(value) => value,
            Err(_) => {
                unsupported = true;
                continue;
            }
        };
        let bytes = source
            .record(TRANSFER, &format!("runtime-state/{name}"), 2 * 1024 * 1024)?
            .ok_or_else(|| error("source_changed"))?;
        json(&bytes)?;
        if unsupported_version(&bytes, &[2])? {
            unsupported = true;
        }
        let parsed = serde_json::from_slice::<InstallRecord>(&bytes)
            .ok()
            .filter(|record| {
                crate::install_state_validation::validate_record(config, record).is_ok()
            });
        if parsed.is_none() {
            invalid = true;
        }
        if temporary {
            pending = true;
        } else {
            record = parsed;
        }
    }
    let (status, reason) = if unsupported {
        (Status::Unsupported, "installation_format")
    } else if invalid {
        (Status::Inconsistent, "installation_invalid")
    } else if pending {
        (Status::Inconsistent, "installation_publication_pending")
    } else if let Some(record) = record.as_ref() {
        match binding.map(|binding| binding.installation_ready(Some(record))) {
            None => (
                Status::Inconsistent,
                "installation_binding_missing_or_invalid",
            ),
            Some(Err(_)) => (Status::Inconsistent, "installation_binding_mismatch"),
            Some(Ok(true)) => (Status::Recognized, "installation_completed_metadata"),
            Some(Ok(false)) => (
                Status::Inconsistent,
                match record {
                    InstallRecord::Started { .. } => "installation_started",
                    InstallRecord::Indeterminate { .. } => "installation_indeterminate",
                    InstallRecord::Failed { .. } => "installation_failed",
                    _ => "installation_restore_pending",
                },
            ),
        }
    } else {
        (Status::Inconsistent, "installation_missing")
    };
    Ok(Component::new(status, reason, INSTALL_CHECKS))
}
