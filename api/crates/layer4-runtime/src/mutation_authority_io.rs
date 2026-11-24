use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::{corrupt, io_error, Disposition, Record, ResourceKey, MAX_AUTHORITY_BYTES};
use crate::failure::{Result, RuntimeError};

const DIRECTORY: &str = "mutation-authority";
const TEMP_PREFIX: &str = ".authority-";
const TEMP_SUFFIX: &str = ".tmp";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);

pub(super) struct AuthorityFile {
    pub path: PathBuf,
    pub record: Record,
}

#[derive(Clone, Copy)]
pub(super) enum TemporaryFiles {
    Remove,
    Preserve,
    Refuse,
}

pub(super) fn scan(
    state_root: &Path,
    budget: &mut crate::mutation_recovery::ScanBudget,
    temporary: TemporaryFiles,
) -> Result<Vec<AuthorityFile>> {
    let directory = state_root.join(DIRECTORY);
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("inspect mutation authority directory", error)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(corrupt("mutation authority path is not a real directory")),
    }
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| io_error("list mutation authority directory", error))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| io_error("read mutation authority entry", error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| corrupt("mutation authority filename is not UTF-8"))?;
        let path = entry.path();
        if !matches!(temporary, TemporaryFiles::Remove) {
            budget.entry()?;
        }
        if temporary_name(&name) {
            match temporary {
                TemporaryFiles::Remove => {
                    crate::fs_durability::remove_file(&path).map_err(|error| {
                        io_error("remove incomplete mutation authority record", error)
                    })?
                }
                TemporaryFiles::Preserve => {}
                TemporaryFiles::Refuse => {
                    return Err(RuntimeError::unavailable(
                        "agent_process_authority_unsettled",
                        "Agent process proof is unavailable while authority publication is pending",
                    ));
                }
            }
            continue;
        }
        if !canonical_name(&name) {
            return Err(corrupt(
                "mutation authority directory contains an unknown file",
            ));
        }
        if matches!(temporary, TemporaryFiles::Remove) {
            budget.entry()?;
        }
        files.push(AuthorityFile {
            path: path.clone(),
            record: read_record(&path, budget)?,
        });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

/// Run only after the entire authority/journal inventory and admission validate.
/// These names were never published as authority; remove them before consuming
/// append space so crash leftovers cannot exhaust the physical recovery bound.
pub(super) fn clear_unpublished(config: &crate::config::RuntimeConfig) -> Result<()> {
    let mut budget = crate::mutation_recovery::ScanBudget::new(
        config,
        crate::mutation_recovery::authority_limit(config),
    );
    let mut paths = Vec::new();
    for root in crate::mutation_scope::roots(config)? {
        let entries = match std::fs::read_dir(root.join(DIRECTORY)) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io_error("list mutation authority directory", error)),
        };
        for entry in entries {
            budget.entry()?;
            let entry = entry.map_err(|error| io_error("read mutation authority entry", error))?;
            if entry.file_name().to_str().is_some_and(temporary_name) {
                paths.push(entry.path());
            }
        }
    }
    for path in paths {
        crate::fs_durability::remove_file(&path)
            .map_err(|error| io_error("remove unpublished mutation authority", error))?;
    }
    Ok(())
}

pub(super) fn persist_new(state_root: &Path, record: &Record) -> Result<()> {
    let directory = prepare_directory(state_root)?;
    let canonical = record_path(state_root, record);
    let temporary = temporary_path(&directory);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| io_error("create mutation authority temporary file", error))?;
    let bytes = serde_json::to_vec(record)
        .map_err(|error| corrupt(&format!("encode mutation authority record: {error}")))?;
    if bytes.len() as u64 > MAX_AUTHORITY_BYTES {
        let _ = crate::fs_durability::remove_file(&temporary);
        return Err(corrupt("mutation authority record is oversized"));
    }
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = crate::fs_durability::remove_file(&temporary);
        return Err(io_error("write mutation authority record", error));
    }
    drop(file);
    match crate::fs_durability::hard_link(&temporary, &canonical) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            crate::fs_durability::remove_file(&temporary).map_err(|cleanup| {
                io_error("remove raced mutation authority temporary file", cleanup)
            })?;
            return Err(RuntimeError::rejected(
                "mutation_authority_raced",
                "mutation authority was admitted concurrently",
            ));
        }
        Err(error) => {
            let _ = crate::fs_durability::remove_file(&temporary);
            return Err(io_error("commit mutation authority record", error));
        }
    }
    crate::fs_durability::remove_file(&temporary)
        .map_err(|error| io_error("remove mutation authority temporary file", error))
}

pub(super) fn sync_record(path: &Path) -> Result<()> {
    crate::fs_durability::sync_file(path)
        .map_err(|error| io_error("sync retained mutation authority", error))?;
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("mutation authority has no parent"))?;
    crate::fs_durability::sync_directory(parent)
        .map_err(|error| io_error("sync retained mutation authority directory", error))
}

fn read_record(path: &Path, budget: &mut crate::mutation_recovery::ScanBudget) -> Result<Record> {
    let mut file =
        open_no_follow(path).map_err(|error| io_error("open mutation authority record", error))?;
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect mutation authority record", error))?;
    if !metadata.is_file() || metadata.len() > MAX_AUTHORITY_BYTES {
        return Err(corrupt("mutation authority record is unsafe or oversized"));
    }
    budget.bytes(metadata.len())?;
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_AUTHORITY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read mutation authority record", error))?;
    if bytes.len() as u64 > MAX_AUTHORITY_BYTES {
        return Err(corrupt("mutation authority record is oversized"));
    }
    serde_json::from_slice(&bytes)
        .map_err(|error| corrupt(&format!("decode mutation authority record: {error}")))
}

fn prepare_directory(state_root: &Path) -> Result<PathBuf> {
    let directory = state_root.join(DIRECTORY);
    crate::fs_durability::create_dir_all(&directory)
        .map_err(|error| io_error("create mutation authority directory", error))?;
    let metadata = std::fs::symlink_metadata(&directory)
        .map_err(|error| io_error("inspect mutation authority directory", error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(corrupt("mutation authority path is not a real directory"));
    }
    Ok(directory)
}

pub(super) fn record_path(state_root: &Path, record: &Record) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(b"chrono.mutation.authority.record.v1\0");
    match &record.resource {
        ResourceKey::Worktree { workspace, name } => {
            digest.update(b"worktree\0");
            field(&mut digest, workspace);
            field(&mut digest, name);
        }
        ResourceKey::AgentSkill { agent_id, skill_id } => {
            digest.update(b"agent_skill\0");
            field(&mut digest, agent_id);
            field(&mut digest, skill_id);
        }
        ResourceKey::AgentHome { agent_id } => {
            digest.update(b"agent_home\0");
            field(&mut digest, agent_id);
        }
        ResourceKey::AgentProcess {
            agent_id,
            incarnation,
        } => {
            digest.update(b"agent_process\0");
            field(&mut digest, agent_id);
            if let Some(incarnation) = incarnation {
                digest.update(incarnation.to_be_bytes());
            }
        }
    }
    digest.update(record.authority_fence.to_be_bytes());
    digest.update([match record.disposition {
        Disposition::Present => 1,
        Disposition::Absent => 2,
    }]);
    let name = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    state_root.join(DIRECTORY).join(format!("{name}.json"))
}

fn field(digest: &mut Sha256, value: &str) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value.as_bytes());
}

fn temporary_path(directory: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    directory.join(format!(
        "{TEMP_PREFIX}{}-{sequence}{TEMP_SUFFIX}",
        std::process::id()
    ))
}

fn open_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    options.open(path)
}

fn canonical_name(name: &str) -> bool {
    name.strip_suffix(".json")
        .is_some_and(|stem| stem.len() == 64 && stem.bytes().all(lower_hex))
}

fn temporary_name(name: &str) -> bool {
    name.starts_with(TEMP_PREFIX)
        && name.ends_with(TEMP_SUFFIX)
        && name.len() <= 128
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}
