use super::io::{Source, STATE, TRANSFER};
use super::{error, sha256, unsupported_version, Component, Status, RECORD_BYTES};
use crate::config::RuntimeConfig;
use crate::failure::Result;
use crate::ledger_import::inventory::{classify, Kind};

const MUTATION_MANIFEST: &str = "mutation-ledger-v2.current.json";
const RETIREMENT_MANIFEST: &str = "mutation-retirement-v1.current.json";
const MUTATION_CHECKS: &str = "descriptor_original_roots_scope_barriers_store_entry_metadata";
const RETIREMENT_CHECKS: &str =
    "descriptor_original_roots_active_hash_barrier_store_entry_metadata";

pub(super) fn mutation(
    config: &RuntimeConfig,
    source: &mut Source,
) -> Result<(Component, Option<String>)> {
    let names = source.names(STATE, "")?.unwrap_or_default();
    let bytes = source.record(STATE, MUTATION_MANIFEST, RECORD_BYTES)?;
    let digest = bytes.as_deref().map(sha256);
    let mut scopes = vec![(0, String::new())];
    let mut invalid_scope = false;
    let mut future = false;
    for name in &names {
        if let Some(number) = name.strip_prefix("process-generation-") {
            match number.parse::<u64>() {
                Ok(generation)
                    if generation != 0
                        && generation <= config.generation
                        && generation.to_string() == number =>
                {
                    let children = source.names(STATE, name)?.unwrap_or_default();
                    if children
                        .iter()
                        .any(|child| child != "mutation-authority" && child != "runtime-mutations")
                    {
                        invalid_scope = true;
                    }
                    scopes.push((generation, format!("{name}/")));
                }
                _ => invalid_scope = true,
            }
        }
        if (name.starts_with("mutation-ledger-") || name.starts_with(".mutation-ledger-"))
            && !matches!(
                name.as_str(),
                MUTATION_MANIFEST
                    | "mutation-ledger-v2"
                    | "mutation-ledger-v2.lock"
                    | ".mutation-ledger-v2.preparing"
                    | ".mutation-ledger-v2.current.tmp"
                    | ".mutation-ledger-v2.fresh.json"
            )
        {
            future = true;
        }
    }
    let mut partial = false;
    for name in [
        ".mutation-ledger-v2.preparing",
        ".mutation-ledger-v2.current.tmp",
        ".mutation-ledger-v2.fresh.json",
    ] {
        partial |= publication(source, name)?;
    }
    let decoded = bytes
        .as_deref()
        .and_then(|bytes| crate::ledger_format::inspect_descriptor(config, bytes).ok());
    let mut orphan = false;
    let mut barriers = true;
    let mut legacy = true;
    for (generation, prefix) in &scopes {
        for (directory, kind) in [
            ("mutation-authority", Kind::Authority),
            ("runtime-mutations", Kind::Journal),
        ] {
            let path = format!("{prefix}{directory}");
            let children = source.names(STATE, &path)?.unwrap_or_default();
            for name in &children {
                let file = format!("{path}/{name}");
                if name == crate::ledger_format::BARRIER {
                    orphan |= bytes.is_none();
                    let captured = source.record(STATE, &file, RECORD_BYTES)?;
                    barriers &= decoded
                        .as_ref()
                        .is_some_and(|(_, canonical)| captured.as_ref() == Some(canonical));
                } else {
                    legacy &= classify(name, kind).is_ok();
                    // No raw journal data is read or returned by this command.
                    source.file_size(STATE, &file)?;
                }
            }
            if let Some((born, _)) = &decoded {
                if *generation == 0 || *generation == *born || *generation == config.generation {
                    barriers &= children == [crate::ledger_format::BARRIER];
                } else {
                    barriers &= children.is_empty() || children == [crate::ledger_format::BARRIER];
                }
            }
        }
    }
    for name in source
        .names(STATE, "workspace-operations")?
        .unwrap_or_default()
    {
        legacy &= classify(&name, Kind::Workspace).is_ok();
        source.file_size(STATE, &format!("workspace-operations/{name}"))?;
    }
    let transfer = source.record(
        TRANSFER,
        &format!("runtime-state/{}", crate::ledger_format::BARRIER),
        RECORD_BYTES,
    )?;
    orphan |= transfer.is_some() && bytes.is_none();
    if let Some((born, canonical)) = &decoded {
        barriers &= transfer.as_ref() == Some(canonical)
            && scopes.iter().any(|(generation, _)| generation == born)
            && scopes
                .iter()
                .any(|(generation, _)| *generation == config.generation);
    }
    let store = store(source, "mutation-ledger-v2")?;
    let lock = source.file_size(STATE, "mutation-ledger-v2.lock")?;
    let (status, reason) = if future {
        (Status::Unsupported, "mutation_artifacts_unknown")
    } else if invalid_scope {
        (Status::Inconsistent, "mutation_scope_invalid")
    } else if let Some(bytes) = bytes {
        if unsupported_version(&bytes, &[2])? {
            (Status::Unsupported, "mutation_version")
        } else if decoded.is_none() {
            (Status::Inconsistent, "mutation_descriptor_invalid")
        } else if partial {
            (Status::Inconsistent, "mutation_publication_pending")
        } else if !legacy {
            (Status::Unsupported, "mutation_v1_entries")
        } else if !barriers {
            (Status::Inconsistent, "mutation_barriers_incomplete")
        } else if lock != Some(0) {
            (Status::Inconsistent, "mutation_writer_metadata_missing")
        } else if let Some(reason) = store {
            (Status::Inconsistent, reason)
        } else {
            (Status::Recognized, "mutation_v2_metadata")
        }
    } else if partial || orphan || lock.is_some() || source.present(STATE, "mutation-ledger-v2")? {
        (Status::Inconsistent, "mutation_manifest_missing")
    } else if !legacy {
        (Status::Unsupported, "mutation_v1_entries")
    } else {
        (Status::Recognized, "mutation_v1_layout")
    };
    Ok((Component::new(status, reason, MUTATION_CHECKS), digest))
}

pub(super) fn retirement(
    config: &RuntimeConfig,
    source: &mut Source,
    active: Option<&str>,
) -> Result<Component> {
    let bytes = source.record(STATE, RETIREMENT_MANIFEST, RECORD_BYTES)?;
    let marker = source.record(
        TRANSFER,
        &format!("runtime-state/{}", crate::retirement_format::BARRIER),
        RECORD_BYTES,
    )?;
    let mut partial = false;
    for name in [
        ".mutation-retirement-v1.preparing",
        ".mutation-retirement-v1.current.tmp",
    ] {
        partial |= publication(source, name)?;
    }
    let names = source.names(STATE, "")?.unwrap_or_default();
    let future = names.iter().any(|name| {
        (name.starts_with("mutation-retirement-") || name.starts_with(".mutation-retirement-"))
            && !matches!(
                name.as_str(),
                RETIREMENT_MANIFEST
                    | "mutation-retirement-v1"
                    | ".mutation-retirement-v1.preparing"
                    | ".mutation-retirement-v1.current.tmp"
            )
    });
    let store = store(source, "mutation-retirement-v1")?;
    let (status, reason) = if future {
        (Status::Unsupported, "retirement_artifacts_unknown")
    } else if let Some(bytes) = bytes {
        if unsupported_version(&bytes, &[1, 2, 3])? {
            (Status::Unsupported, "retirement_version")
        } else if crate::retirement_format::inspect_descriptor(config, &bytes, active).is_err() {
            (Status::Inconsistent, "retirement_descriptor_invalid")
        } else if marker.as_ref() != Some(&bytes) {
            (
                Status::Inconsistent,
                "retirement_barrier_missing_or_changed",
            )
        } else if partial {
            (Status::Inconsistent, "retirement_publication_pending")
        } else if let Some(reason) = store {
            (Status::Inconsistent, reason)
        } else {
            (
                Status::Recognized,
                "retirement_metadata_transition_unassessed",
            )
        }
    } else if marker.is_some() || partial || source.present(STATE, "mutation-retirement-v1")? {
        (Status::Inconsistent, "retirement_manifest_missing")
    } else {
        (Status::Absent, "retirement_not_activated")
    };
    Ok(Component::new(status, reason, RETIREMENT_CHECKS))
}

fn store(source: &mut Source, root: &str) -> Result<Option<&'static str>> {
    let Some(names) = source.names(STATE, root)? else {
        return Ok(Some("store_missing"));
    };
    let mut sizes = std::collections::BTreeMap::new();
    for name in &names {
        let size = source
            .file_size(STATE, &format!("{root}/{name}"))?
            .ok_or_else(|| error("source_changed"))?;
        sizes.insert(name.as_str(), size);
    }
    if names.iter().any(|name| {
        !matches!(
            name.as_str(),
            "writer.lock"
                | "ledger.sqlite3"
                | "emergency.reserve"
                | "ledger.sqlite3-journal"
                | "ledger.sqlite3-wal"
                | "ledger.sqlite3-shm"
        )
    }) {
        return Ok(Some("store_entries_unsupported"));
    }
    if sizes.get("ledger.sqlite3").is_none_or(|size| *size == 0)
        || sizes.get("writer.lock") != Some(&0)
        || !sizes.contains_key("emergency.reserve")
    {
        return Ok(Some("store_dependency_incomplete"));
    }
    if names.iter().any(|name| name.starts_with("ledger.sqlite3-")) {
        return Ok(Some("store_recovery_unassessed"));
    }
    Ok(None)
}

fn publication(source: &mut Source, name: &str) -> Result<bool> {
    if name.ends_with(".preparing") {
        let present = source.names(STATE, name)?.is_some();
        if present {
            store(source, name)?;
        }
        Ok(present)
    } else {
        Ok(source.record(STATE, name, RECORD_BYTES)?.is_some())
    }
}
