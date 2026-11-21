//! Preserve an ambiguous, never-published checkout and prepare the exact same
//! Workspace afresh. Only explicit admission can enter this path.
use std::path::{Path, PathBuf};

use layer3_adapter::{
    WorkspacePrepareRecoveryReceipt, WorkspacePrepareRecoveryRequest, WorkspaceReceipt,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::RuntimeConfig;
use crate::failure::{Result, RuntimeError};
#[path = "workspace_prepare_recovery_io.rs"]
mod io;
#[path = "workspace_prepare_recovery_publish.rs"]
mod publish;
use io::{
    append, directory, ensure_directory, invalid, io_error, read, require_absent, DirectoryIdentity,
};
use publish::prepare_and_publish;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    version: u8,
    sandbox_id: String,
    home_identity: String,
    request: layer3_adapter::WorkspaceRequest,
    journal_sha256: String,
    directory: DirectoryIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    version: u8,
    sequence: u32,
    previous_stored_operation_id: Option<String>,
    request: WorkspacePrepareRecoveryRequest,
    original_journal_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Terminal {
    Completed {
        receipt: WorkspacePrepareRecoveryReceipt,
    },
    Failed {
        code: String,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    directory: DirectoryIdentity,
    resolved_commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_commit: Option<String>,
}

struct Paths {
    evidence: PathBuf,
    attempts: PathBuf,
    preserved: PathBuf,
    staged: PathBuf,
    published: PathBuf,
    candidate_root: PathBuf,
    candidate: PathBuf,
}

impl Paths {
    fn new(config: &RuntimeConfig, request: &WorkspacePrepareRecoveryRequest) -> Self {
        let workspace = &request.original.workspace_id;
        let digest = operation_hash(&request.operation_id);
        let evidence = config
            .state_root
            .join("workspace-prepare-recovery")
            .join(workspace);
        let retained = config.home.join(".cma/prepare-recovery").join(workspace);
        let candidate_root = retained.join("attempts").join(&digest);
        Self {
            attempts: evidence.join("attempts"),
            evidence,
            preserved: retained.join("original"),
            staged: config.home.join(".cma/preparing").join(workspace),
            published: config.home.join(&request.original.name),
            candidate: candidate_root.join(workspace),
            candidate_root,
        }
    }
    fn attempt(&self, request: &WorkspacePrepareRecoveryRequest) -> PathBuf {
        self.attempts.join(operation_hash(&request.operation_id))
    }
}

/// The caller holds the operation gate and current boot's cross-process writer
/// guard for this entire detached operation, including after caller disconnect.
pub(crate) async fn execute(
    config: &RuntimeConfig,
    request: &WorkspacePrepareRecoveryRequest,
) -> Result<WorkspacePrepareRecoveryReceipt> {
    request
        .validate()
        .map_err(|error| invalid(&error.to_string()))?;
    crate::paths::safe_component(&request.original.name, "workspace name")?;
    let home_identity = config
        .home_identity
        .clone()
        .ok_or_else(|| invalid("persistent home identity is required"))?;
    let journal_sha256 =
        crate::workspace_journal::indeterminate_prepare_digest(config, &request.original)?;
    let paths = Paths::new(config, request);
    for path in [
        config.home.join(".cma"),
        config.home.join(".cma/prepare-recovery"),
        paths.preserved.parent().unwrap().to_path_buf(),
        config.state_root.join("workspace-prepare-recovery"),
        paths.evidence.clone(),
        paths.attempts.clone(),
    ] {
        ensure_directory(&path)?;
    }
    let original_path = paths.evidence.join("original.json");
    let original = match read::<Original>(&original_path)? {
        Some(original) => original,
        None => {
            require_absent(&paths.published)?;
            require_absent(&paths.preserved)?;
            directory(&config.home.join(".cma/preparing"))?
                .ok_or_else(|| invalid("original staging root is absent"))?;
            let identity = directory(&paths.staged)?
                .ok_or_else(|| invalid("original staging directory is absent"))?;
            require_incomplete_original(&paths.staged)?;
            let original = Original {
                version: 1,
                sandbox_id: config.sandbox_id.clone(),
                home_identity: home_identity.clone(),
                request: request.original.clone(),
                journal_sha256: journal_sha256.clone(),
                directory: identity,
                source_commit: original_source_commit(&paths.staged, request)?,
            };
            append(&original_path, &original)?;
            original
        }
    };
    if original.version != 1
        || original.sandbox_id != config.sandbox_id
        || original.home_identity != home_identity
        || original.request != request.original
        || original.journal_sha256 != journal_sha256
    {
        return Err(invalid(
            "recovery original admission or retained home changed",
        ));
    }
    preserve_original(&paths, &original)?;
    let source_commit = original_source_commit(&paths.preserved, request)?;
    if original.source_commit.is_some() && source_commit != original.source_commit {
        return Err(invalid("original source pin changed"));
    }
    if let Some(pin) = &source_commit {
        append(&paths.evidence.join("source-commit.json"), pin)?;
    }
    let attempt_dir = paths.attempt(request);
    let attempt = admit(&paths, request, &journal_sha256)?;
    if let Some(terminal) = read::<Terminal>(&attempt_dir.join("terminal.json"))? {
        return terminal_result(&paths, request, &journal_sha256, terminal);
    }
    require_absent(&paths.published).or_else(|error| {
        if read::<Publication>(&attempt_dir.join("publication.json"))?.is_some() {
            Ok(())
        } else {
            Err(error)
        }
    })?;
    let result = prepare_and_publish(
        config,
        request,
        &paths,
        &attempt_dir,
        &journal_sha256,
        source_commit.as_deref(),
    )
    .await;
    match result {
        Ok(receipt) => {
            append(
                &attempt_dir.join("terminal.json"),
                &Terminal::Completed {
                    receipt: receipt.clone(),
                },
            )?;
            Ok(receipt)
        }
        Err(error) if error.code == "workspace_recovery_io_failed" => Err(error),
        Err(error) if read::<Publication>(&attempt_dir.join("publication.json"))?.is_some() => {
            Err(RuntimeError::unavailable(
                "workspace_recovery_publication_pending",
                format!(
                    "prepared Workspace publication requires reconciliation: {}",
                    error.code
                ),
            ))
        }
        Err(error) => {
            // A failed or interrupted candidate is retained in its own attempt.
            // A new operation must explicitly acknowledge this terminal attempt.
            let detail = format!("Workspace recovery attempt failed ({}); original and attempted checkout files are preserved", error.code);
            let terminal = Terminal::Failed {
                code: "workspace_recovery_attempt_failed".into(),
                detail,
            };
            append(&attempt_dir.join("terminal.json"), &terminal)?;
            tracing::warn!(operation_id = %attempt.request.operation_id, detail = %error, "Workspace preparation recovery attempt failed");
            terminal_result(&paths, request, &journal_sha256, terminal)
        }
    }
}

fn original_source_commit(
    path: &Path,
    request: &WorkspacePrepareRecoveryRequest,
) -> Result<Option<String>> {
    crate::repository_snapshot::source_commit(path, &request.original.source)
        .map_err(|_| invalid("original repository source pin cannot be established"))
}

fn require_incomplete_original(path: &Path) -> Result<()> {
    directory(&path.join(".git"))?
        .ok_or_else(|| invalid("original stage has no real Git directory"))?;
    directory(&path.join(".git/info"))?
        .ok_or_else(|| invalid("original stage has no real Git info directory"))?;
    require_absent(&path.join(".git/info/cma-workspace-id"))
}

fn preserve_original(paths: &Paths, original: &Original) -> Result<()> {
    match (directory(&paths.staged)?, directory(&paths.preserved)?) {
        (Some(staged), None) if staged == original.directory => {
            require_absent(&paths.published)?;
            require_incomplete_original(&paths.staged)?;
            crate::fs_durability::rename(&paths.staged, &paths.preserved).map_err(io_error)?;
        }
        (None, Some(preserved)) if preserved == original.directory => {}
        _ => {
            return Err(invalid(
                "original staged checkout changed or its preserved archive is ambiguous",
            ))
        }
    }
    if directory(&paths.preserved)?.as_ref() != Some(&original.directory) {
        return Err(invalid(
            "original directory identity changed during preservation",
        ));
    }
    require_incomplete_original(&paths.preserved)
}

fn admit(
    paths: &Paths,
    request: &WorkspacePrepareRecoveryRequest,
    digest: &str,
) -> Result<Attempt> {
    let mut attempts = Vec::new();
    for entry in std::fs::read_dir(&paths.attempts).map_err(io_error)? {
        if attempts.len() >= 128 {
            return Err(invalid("Workspace recovery attempt limit reached"));
        }
        let entry = entry.map_err(io_error)?;
        directory(&entry.path())?
            .ok_or_else(|| invalid("recovery attempt directory disappeared"))?;
        let Some(attempt) = read::<Attempt>(&entry.path().join("request.json"))? else {
            continue;
        };
        attempt
            .request
            .validate()
            .map_err(|_| invalid("stored recovery request is invalid"))?;
        if attempt.version != 1
            || attempt.original_journal_sha256 != digest
            || attempt.request.original != request.original
            || entry.file_name().to_str() != Some(&operation_hash(&attempt.request.operation_id))
        {
            return Err(invalid(
                "stored recovery attempt does not match original admission",
            ));
        }
        attempts.push(attempt);
    }
    attempts.sort_by_key(|attempt| attempt.sequence);
    let mut previous: Option<&str> = None;
    for (index, attempt) in attempts.iter().enumerate() {
        if attempt.sequence as usize != index + 1
            || attempt.previous_stored_operation_id.as_deref() != previous
        {
            return Err(invalid(
                "stored recovery attempts do not form one ordered chain",
            ));
        }
        previous = Some(&attempt.request.operation_id);
    }
    if let Some(existing) = attempts
        .iter()
        .find(|attempt| attempt.request.operation_id == request.operation_id)
    {
        if &existing.request != request {
            return Err(invalid("recovery replay changed its original request"));
        }
        if attempts.last() != Some(existing)
            && !matches!(
                read::<Terminal>(&paths.attempt(request).join("terminal.json"))?,
                Some(Terminal::Failed { .. })
            )
        {
            return Err(invalid(
                "an older nonterminal recovery cannot execute after a successor",
            ));
        }
        return Ok(existing.clone());
    }
    require_absent(&paths.published)?;
    if let Some(latest) = attempts.last() {
        if !matches!(
            read::<Terminal>(&paths.attempt(&latest.request).join("terminal.json"))?,
            Some(Terminal::Failed { .. })
        ) {
            return Err(invalid("previous recovery has no terminal failure proof"));
        }
        let supplied = request
            .previous_recovery_operation_id
            .as_deref()
            .ok_or_else(|| invalid("retry did not name its preceding attempt"))?;
        if supplied != latest.request.operation_id
            && (attempts.iter().any(|attempt| {
                attempt.request.operation_id == supplied
                    || attempt.request.previous_recovery_operation_id.as_deref() == Some(supplied)
            }) || paths.attempts.join(operation_hash(supplied)).exists())
        {
            return Err(invalid(
                "retry names a stale or ambiguous preceding attempt",
            ));
        }
    }
    // Missing preceding runtime admission is allowed only when it has never
    // touched a candidate; this covers failure before transport reached runtime.
    if let Some(previous) = &request.previous_recovery_operation_id {
        if !attempts
            .iter()
            .any(|attempt| &attempt.request.operation_id == previous)
        {
            let candidates = paths
                .candidate_root
                .parent()
                .unwrap()
                .join(operation_hash(previous));
            require_absent(&candidates)?;
            require_absent(&paths.attempts.join(operation_hash(previous)))?;
        }
    }
    let attempt = Attempt {
        version: 1,
        sequence: u32::try_from(attempts.len() + 1).map_err(|_| invalid("too many attempts"))?,
        previous_stored_operation_id: previous.map(str::to_owned),
        request: request.clone(),
        original_journal_sha256: digest.into(),
    };
    let path = paths.attempt(request);
    ensure_directory(&path)?;
    append(&path.join("request.json"), &attempt)?;
    Ok(attempt)
}

fn terminal_result(
    paths: &Paths,
    request: &WorkspacePrepareRecoveryRequest,
    digest: &str,
    terminal: Terminal,
) -> Result<WorkspacePrepareRecoveryReceipt> {
    match terminal {
        Terminal::Failed { code, detail } if code == "workspace_recovery_attempt_failed" => Err(
            RuntimeError::rejected("workspace_recovery_attempt_failed", detail),
        ),
        Terminal::Failed { .. } => Err(invalid("stored recovery terminal failure is invalid")),
        Terminal::Completed { receipt } => {
            let publication: Publication = read(&paths.attempt(request).join("publication.json"))?
                .ok_or_else(|| invalid("completed recovery has no publication proof"))?;
            if receipt.prepared.operation_id != request.operation_id
                || receipt.prepared.request_sha256 != request.request_sha256
                || receipt.prepared.workspace_id != request.original.workspace_id
                || receipt.prepared.name != request.original.name
                || receipt.prepared.path != paths.published.to_string_lossy()
                || receipt.preserved_path != paths.preserved.to_string_lossy()
                || receipt.original_journal_sha256 != digest
                || receipt.prepared.resolved_commit != publication.resolved_commit
                || receipt.prepared.source_commit != publication.source_commit
                || publication.source_commit != original_source_commit(&paths.preserved, request)?
                || directory(&paths.published)?.as_ref() != Some(&publication.directory)
            {
                return Err(invalid(
                    "completed recovery does not prove its exact published Workspace",
                ));
            }
            Ok(receipt)
        }
    }
}

fn operation_hash(operation: &str) -> String {
    format!("{:x}", Sha256::digest(operation.as_bytes()))
}

#[cfg(test)]
#[path = "workspace_prepare_recovery_tests.rs"]
mod tests;
