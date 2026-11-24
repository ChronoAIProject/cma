//! Offline structural assessment only. No startup, database open or restore authority.
mod components;
mod formats;
mod io;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
use crate::ledger_import::{Budget, Limits};

const REQUEST_BYTES: u64 = 8192;
const RECORD_BYTES: u64 = 8192;
const OUTPUT_BYTES: usize = 16384;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u8,
    sandbox_id: String,
    home_identity: String,
    generation: u64,
    layout: String,
}

impl Request {
    fn config(&self) -> Result<RuntimeConfig> {
        if self.version != 1 || self.layout != "native-runtime-v1" {
            return Err(error("request_unsupported"));
        }
        layer3_adapter::RuntimeStatusRequest {
            sandbox_id: self.sandbox_id.clone(),
            home_identity: self.home_identity.clone(),
            generation: self.generation,
        }
        .validate()
        .map_err(|_| error("request_invalid"))?;
        crate::paths::safe_component(&self.sandbox_id, "source")
            .map_err(|_| error("request_invalid"))?;
        Ok(RuntimeConfig {
            bind: "127.0.0.1:0".parse().expect("literal address"),
            sandbox_id: self.sandbox_id.clone(),
            generation: self.generation,
            home_identity: Some(self.home_identity.clone()),
            home: PathBuf::from("/home/cma"),
            state_root: PathBuf::from("/var/lib/cma-runtime").join(&self.sandbox_id),
            checkpoint_root: PathBuf::from("/var/lib/cma-transfer/cma-checkpoints"),
            install_env: BTreeMap::new(),
            process_env: BTreeMap::new(),
            model_proxy: None,
            model_credit_required: false,
            platform_proxy: None,
            tool_proxy: None,
            github_proxy: None,
            desktop_supervision_enabled: false,
            nyxid_url: None,
            allow_local_git: false,
            machine_included: false,
            extended_desktop_startup: false,
            capacity_recovery_enabled: false,
            legacy_journal_recovery_enabled: false,
            agent_process_restart_enabled: false,
            preserve_file_mode: false,
        })
    }
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum Status {
    Recognized,
    Inconsistent,
    Unsupported,
    Absent,
}

#[derive(Serialize, Debug)]
struct Component {
    status: Status,
    // Only static codes from component checks enter the report.
    reason: &'static str,
    checks: &'static str,
}

impl Component {
    fn new(status: Status, reason: &'static str, checks: &'static str) -> Self {
        Self {
            status,
            reason,
            checks,
        }
    }
}

#[derive(Serialize)]
struct Report {
    format: &'static str,
    complete: bool,
    restore_admission: &'static str,
    semantic_reconciliation: &'static str,
    writer_exclusion: &'static str,
    provider_identity: &'static str,
    external_immutability: &'static str,
    database_contents: &'static str,
    retirement_transition_readiness: &'static str,
    native_session_recovery: &'static str,
    full_home_hash: &'static str,
    legacy_record_contents: &'static str,
    request_sha256: String,
    assessed_metadata_sha256: String,
    examined_entries: usize,
    metadata_bytes_read: u64,
    binding: Component,
    installation: Component,
    mutation: Component,
    retirement: Component,
}

pub(crate) fn run(arguments: &[OsString]) -> std::result::Result<(), String> {
    if arguments.len() != 1 {
        return Err("restored_home_request_arguments".into());
    }
    let bytes = execute(Path::new(&arguments[0])).map_err(|error| error.code)?;
    std::io::stdout()
        .lock()
        .write_all(&bytes)
        .map_err(|_| "restored_home_output_io".to_owned())
}

fn execute(path: &Path) -> Result<Vec<u8>> {
    let mut budget = budget();
    let bytes = io::request(path, &mut budget)?;
    let request: Request = serde_json::from_slice(&bytes).map_err(|_| error("request_invalid"))?;
    let config = request.config()?;
    let mut source = io::Source::open(&config, budget)?;
    assess(&config, &mut source, &sha256(&bytes), OUTPUT_BYTES)
}

fn assess(
    config: &RuntimeConfig,
    source: &mut io::Source,
    request_digest: &str,
    output_limit: usize,
) -> Result<Vec<u8>> {
    let (binding, installation) = components::assess(config, source)?;
    let (mutation, active) = formats::mutation(config, source)?;
    let retirement = formats::retirement(config, source, active.as_deref())?;
    let digest = source.finish(request_digest)?;
    let report = Report {
        format: "restored-home-assessment-v1",
        complete: true,
        restore_admission: "HELD",
        semantic_reconciliation: "NOT_RUN",
        writer_exclusion: "UNVERIFIED",
        provider_identity: "UNVERIFIED",
        external_immutability: "UNVERIFIED",
        database_contents: "NOT_ASSESSED",
        retirement_transition_readiness: "NOT_ASSESSED",
        native_session_recovery: "NOT_ASSESSED",
        full_home_hash: "NOT_ASSESSED",
        legacy_record_contents: "NOT_ASSESSED",
        request_sha256: request_digest.into(),
        assessed_metadata_sha256: digest,
        examined_entries: source.budget.entries,
        metadata_bytes_read: source.budget.bytes,
        binding,
        installation,
        mutation,
        retirement,
    };
    // The report has only fixed codes, counts and bounded digests; no source text.
    let mut bytes = serde_json::to_vec(&report).map_err(|_| error("output_encoding"))?;
    bytes.push(b'\n');
    if bytes.len() > output_limit {
        return Err(error("output_budget"));
    }
    source.check()?;
    Ok(bytes)
}

fn budget() -> Budget {
    Budget::new(Limits {
        entries: 8192,
        bytes: 16 * 1024 * 1024,
        metadata_bytes: 16 * 1024 * 1024,
        deadline: Duration::from_secs(30),
    })
}

fn sha256(bytes: &[u8]) -> String {
    crate::command::hex(&Sha256::digest(bytes))
}
fn error(code: &'static str) -> RuntimeError {
    RuntimeError::recorded(
        format!("restored_home_{code}"),
        "restored-home assessment did not complete".into(),
        false,
    )
}

fn json(bytes: &[u8]) -> Result<Option<serde_json::Value>> {
    if let Err(failure) = crate::ledger_import::json::check(bytes) {
        return if failure.code == "mutation_ledger_migration_capacity" {
            Err(error("metadata_budget"))
        } else {
            Ok(None)
        };
    }
    Ok(serde_json::from_slice(bytes).ok())
}

fn unsupported_version(bytes: &[u8], versions: &[u64]) -> Result<bool> {
    Ok(json(bytes)?
        .and_then(|value| value.get("version").and_then(|v| v.as_u64()))
        .is_some_and(|version| !versions.contains(&version)))
}
