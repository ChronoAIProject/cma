use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::{corrupt, io_error, Record, Status, MAX_JOURNAL_BYTES, MAX_JOURNAL_ENTRIES};
use crate::failure::{Result, RuntimeError};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const TEMP_PREFIX: &str = ".workspace-journal-";
const TEMP_SUFFIX: &str = ".tmp";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Canonical,
    Temporary,
}

pub(super) struct JournalFile {
    pub path: PathBuf,
    pub record: Record,
    kind: FileKind,
}

pub(super) fn scan(state_root: &Path) -> Result<Vec<JournalFile>> {
    let directory = journal_directory(state_root);
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error("inspect workspace operation journal", error)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => {
            return Err(corrupt(
                "workspace operation journal is not a real directory",
            ))
        }
    }

    let mut files = Vec::new();
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| io_error("list workspace operation journal", error))?;
    for entry in entries {
        if files.len() == MAX_JOURNAL_ENTRIES {
            return Err(corrupt(
                "workspace operation journal contains too many entries",
            ));
        }
        let entry = entry.map_err(|error| io_error("read workspace journal entry", error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| corrupt("workspace operation journal filename is not UTF-8"))?;
        let kind = classify_name(&name)?;
        let path = entry.path();
        let record = read_record(&path)?
            .ok_or_else(|| corrupt("workspace operation journal entry disappeared"))?;
        files.push(JournalFile { path, record, kind });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

/// Reconcile durable replacement files only after the caller validates every record.
pub(super) fn reconcile(files: Vec<JournalFile>) -> Result<Vec<JournalFile>> {
    let mut canonical = BTreeMap::<PathBuf, JournalFile>::new();
    let mut temporary = BTreeMap::<PathBuf, Vec<JournalFile>>::new();
    for file in files {
        let directory = file
            .path
            .parent()
            .ok_or_else(|| corrupt("workspace journal entry has no parent"))?;
        let target = canonical_path(directory, &file.record.operation_id);
        match file.kind {
            FileKind::Canonical => {
                if file.path != target {
                    return Err(corrupt(
                        "workspace journal filename is not canonical for its operation",
                    ));
                }
                if canonical.insert(target, file).is_some() {
                    return Err(corrupt("workspace journal operation is duplicated"));
                }
            }
            FileKind::Temporary => temporary.entry(target).or_default().push(file),
        }
    }

    for (target, mut replacements) in temporary {
        let current = canonical
            .get_mut(&target)
            .ok_or_else(|| corrupt("workspace journal replacement has no canonical record"))?;
        if replacements.iter().any(|candidate| {
            candidate.record.status == Status::Started
                || !same_operation(&candidate.record, &current.record)
        }) {
            return Err(corrupt(
                "workspace journal replacement does not follow its canonical record",
            ));
        }
        let replacement = replacements
            .pop()
            .expect("a temporary journal group is non-empty");
        if replacements
            .iter()
            .any(|candidate| candidate.record != replacement.record)
        {
            return Err(corrupt(
                "workspace journal has conflicting durable replacements",
            ));
        }

        if current.record.status == Status::Started {
            crate::fs_durability::rename(&replacement.path, &target)
                .map_err(|error| io_error("recover workspace journal replacement", error))?;
            current.record = replacement.record;
        } else if current.record != replacement.record {
            return Err(corrupt(
                "workspace journal terminal record conflicts with a replacement",
            ));
        } else {
            remove_temporary(&replacement.path)?;
        }
        for stale in replacements {
            remove_temporary(&stale.path)?;
        }
    }
    Ok(canonical.into_values().collect())
}

fn same_operation(left: &Record, right: &Record) -> bool {
    left.version == right.version
        && left.sandbox_id == right.sandbox_id
        && left.workspace_id == right.workspace_id
        && left.operation_id == right.operation_id
        && left.request_sha256 == right.request_sha256
        && left.kind == right.kind
        && left.request == right.request
}

pub(super) fn read_record(path: &Path) -> Result<Option<Record>> {
    let Some(bytes) = read_bytes(path)? else {
        return Ok(None);
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| corrupt(&format!("decode workspace operation journal: {error}")))
}

pub(super) fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>> {
    let mut file = match open_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("open workspace operation journal entry", error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect workspace journal entry handle", error))?;
    if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
        return Err(corrupt("workspace journal entry is unsafe or oversized"));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read workspace operation journal entry", error))?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(corrupt("workspace journal entry is unsafe or oversized"));
    }
    Ok(Some(bytes))
}

pub(super) fn persist_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = prepare_parent(path)?;
    let mut file = match OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(RuntimeError::rejected(
                "workspace_journal_raced",
                "workspace operation journal was created concurrently",
            ))
        }
        Err(error) => return Err(io_error("create workspace operation journal", error)),
    };
    if let Err(error) = durable_write(&mut file, bytes) {
        drop(file);
        let _ = crate::fs_durability::remove_file(path);
        return Err(error);
    }
    crate::fs_durability::sync_directory(&parent)
        .map_err(|error| io_error("sync workspace operation journal directory", error))
}

pub(super) fn persist_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let _parent = prepare_parent(path)?;
    let temporary = temporary_path(path)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| io_error("create workspace journal temporary file", error))?;
    if let Err(error) = durable_write(&mut file, bytes) {
        drop(file);
        let _ = crate::fs_durability::remove_file(&temporary);
        return Err(error);
    }
    drop(file);
    crate::fs_durability::rename(&temporary, path)
        .map_err(|error| io_error("commit workspace operation journal", error))
}

fn prepare_parent(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("workspace operation journal path has no parent"))?;
    crate::fs_durability::create_dir_all(parent)
        .map_err(|error| io_error("create workspace operation journal", error))?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| io_error("inspect workspace operation journal directory", error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(corrupt("workspace journal path is not a real directory"));
    }
    Ok(parent.to_path_buf())
}

fn durable_write(file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .map_err(|error| io_error("write workspace operation journal", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync workspace operation journal", error))
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

fn classify_name(name: &str) -> Result<FileKind> {
    let canonical = name
        .strip_suffix(".json")
        .is_some_and(|stem| stem.len() == 64 && stem.bytes().all(lower_hex));
    if canonical {
        return Ok(FileKind::Canonical);
    }
    let temporary = name.starts_with(TEMP_PREFIX)
        && name.ends_with(TEMP_SUFFIX)
        && name.len() <= 240
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'));
    if temporary {
        Ok(FileKind::Temporary)
    } else {
        Err(corrupt(
            "workspace operation journal contains an unknown file",
        ))
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("workspace operation journal path has no parent"))?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| corrupt("workspace operation journal filename is invalid"))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        "{TEMP_PREFIX}{stem}-{}-{sequence}{TEMP_SUFFIX}",
        std::process::id()
    )))
}

fn remove_temporary(path: &Path) -> Result<()> {
    crate::fs_durability::remove_file(path)
        .map_err(|error| io_error("remove recovered workspace journal replacement", error))
}

fn journal_directory(state_root: &Path) -> PathBuf {
    state_root.join("workspace-operations")
}

pub(super) fn journal_path(state_root: &Path, operation_id: &str) -> PathBuf {
    canonical_path(&journal_directory(state_root), operation_id)
}

fn canonical_path(directory: &Path, operation_id: &str) -> PathBuf {
    let digest = Sha256::digest(operation_id.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    directory.join(format!("{name}.json"))
}

fn lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}
