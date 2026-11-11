use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::{corrupt, io_error, Record, Status, MAX_JOURNAL_BYTES, MAX_JOURNAL_ENTRIES};
use crate::failure::{Result, RuntimeError};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const TEMP_PREFIX: &str = ".mutation-journal-";
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
        Err(error) => return Err(io_error("inspect runtime mutation journal", error)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(corrupt("runtime mutation journal is not a real directory")),
    }

    let mut files = Vec::new();
    let entries = std::fs::read_dir(&directory)
        .map_err(|error| io_error("list runtime mutation journal", error))?;
    for entry in entries {
        if files.len() == MAX_JOURNAL_ENTRIES {
            return Err(crate::mutation_capacity::exhausted());
        }
        let entry = entry.map_err(|error| io_error("read runtime mutation entry", error))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| corrupt("runtime mutation journal filename is not UTF-8"))?;
        let kind = classify_name(&name)?;
        let path = entry.path();
        let record = read_record(&path)?
            .ok_or_else(|| corrupt("runtime mutation journal entry disappeared"))?;
        files.push(JournalFile { path, record, kind });
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

pub(super) fn reconcile(files: Vec<JournalFile>, apply: bool) -> Result<Vec<JournalFile>> {
    let mut canonical = BTreeMap::<PathBuf, JournalFile>::new();
    let mut temporary = BTreeMap::<PathBuf, Vec<JournalFile>>::new();
    for file in files {
        let directory = file
            .path
            .parent()
            .ok_or_else(|| corrupt("runtime mutation journal entry has no parent"))?;
        let target = canonical_path(directory, &file.record.operation_id);
        match file.kind {
            FileKind::Canonical => {
                if file.path != target {
                    return Err(corrupt(
                        "runtime mutation journal filename is not canonical",
                    ));
                }
                if canonical.insert(target, file).is_some() {
                    return Err(corrupt("runtime mutation journal operation is duplicated"));
                }
            }
            FileKind::Temporary => temporary.entry(target).or_default().push(file),
        }
    }

    for (target, mut replacements) in temporary {
        let current = canonical
            .get_mut(&target)
            .ok_or_else(|| corrupt("runtime mutation replacement has no canonical record"))?;
        for candidate in &replacements {
            validate_replacement(&current.record, &candidate.record)?;
        }
        let replacement = replacements
            .pop()
            .expect("a temporary runtime mutation group is non-empty");
        if replacements
            .iter()
            .any(|candidate| candidate.record != replacement.record)
        {
            return Err(corrupt(
                "runtime mutation journal has conflicting replacements",
            ));
        }

        if current.record.status == Status::Started {
            if apply {
                crate::fs_durability::rename(&replacement.path, &target)
                    .map_err(|error| io_error("recover runtime mutation replacement", error))?;
            }
            current.record = replacement.record;
        } else if current.record != replacement.record {
            return Err(corrupt(
                "runtime mutation terminal record conflicts with a replacement",
            ));
        } else if apply {
            remove_temporary(&replacement.path)?;
        }
        if apply {
            for stale in replacements {
                remove_temporary(&stale.path)?;
            }
        }
    }
    Ok(canonical.into_values().collect())
}

fn same_operation(left: &Record, right: &Record) -> bool {
    left.version == right.version
        && left.sandbox_id == right.sandbox_id
        && left.runtime_generation == right.runtime_generation
        && left.operation_id == right.operation_id
        && left.request_sha256 == right.request_sha256
        && left.payload_sha256 == right.payload_sha256
        && left.subject == right.subject
}

pub(super) fn validate_replacement(current: &Record, candidate: &Record) -> Result<()> {
    if candidate.status == Status::Started || !same_operation(candidate, current) {
        return Err(corrupt(
            "runtime mutation replacement does not follow its canonical record",
        ));
    }
    Ok(())
}

pub(super) fn has_replacements(state_root: &Path, count: &mut usize) -> Result<bool> {
    let directory = journal_directory(state_root);
    match std::fs::symlink_metadata(&directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error("inspect runtime mutation journal", error)),
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(corrupt("runtime mutation journal is not a real directory")),
    }
    for entry in std::fs::read_dir(directory)
        .map_err(|error| io_error("list runtime mutation journal", error))?
    {
        if *count == MAX_JOURNAL_ENTRIES {
            return Err(crate::mutation_capacity::exhausted());
        }
        *count += 1;
        let name = entry
            .map_err(|error| io_error("read runtime mutation entry", error))?
            .file_name()
            .into_string()
            .map_err(|_| corrupt("runtime mutation journal filename is not UTF-8"))?;
        if classify_name(&name)? == FileKind::Temporary {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn read_record(path: &Path) -> Result<Option<Record>> {
    let mut file = match open_no_follow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error("open runtime mutation journal entry", error)),
    };
    let metadata = file
        .metadata()
        .map_err(|error| io_error("inspect runtime mutation journal handle", error))?;
    if !metadata.is_file() || metadata.len() > MAX_JOURNAL_BYTES {
        return Err(corrupt(
            "runtime mutation journal entry is unsafe or oversized",
        ));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(metadata.len()).unwrap_or(0));
    (&mut file)
        .take(MAX_JOURNAL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| io_error("read runtime mutation journal entry", error))?;
    if bytes.len() as u64 > MAX_JOURNAL_BYTES {
        return Err(corrupt(
            "runtime mutation journal entry is unsafe or oversized",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| corrupt(&format!("decode runtime mutation journal: {error}")))
}

pub(super) fn persist_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = prepare_parent(path)?;
    let mut file = match OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(RuntimeError::rejected(
                "mutation_journal_raced",
                "runtime mutation journal was created concurrently",
            ));
        }
        Err(error) => return Err(io_error("create runtime mutation journal", error)),
    };
    if let Err(error) = durable_write(&mut file, bytes) {
        drop(file);
        let _ = crate::fs_durability::remove_file(path);
        return Err(error);
    }
    crate::fs_durability::sync_directory(&parent)
        .map_err(|error| io_error("sync runtime mutation journal directory", error))
}

pub(super) fn persist_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let _parent = prepare_parent(path)?;
    let temporary = temporary_path(path)?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|error| io_error("create runtime mutation temporary file", error))?;
    if let Err(error) = durable_write(&mut file, bytes) {
        drop(file);
        let _ = crate::fs_durability::remove_file(&temporary);
        return Err(error);
    }
    drop(file);
    crate::fs_durability::rename(&temporary, path)
        .map_err(|error| io_error("commit runtime mutation journal", error))
}

fn prepare_parent(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("runtime mutation journal path has no parent"))?;
    crate::fs_durability::create_dir_all(parent)
        .map_err(|error| io_error("create runtime mutation journal", error))?;
    let metadata = std::fs::symlink_metadata(parent)
        .map_err(|error| io_error("inspect runtime mutation journal directory", error))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(corrupt(
            "runtime mutation journal path is not a real directory",
        ));
    }
    Ok(parent.to_path_buf())
}

fn durable_write(file: &mut File, bytes: &[u8]) -> Result<()> {
    file.write_all(bytes)
        .map_err(|error| io_error("write runtime mutation journal", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync runtime mutation journal", error))
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
        Err(corrupt("runtime mutation journal contains an unknown file"))
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| corrupt("runtime mutation journal path has no parent"))?;
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| corrupt("runtime mutation journal filename is invalid"))?;
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(
        "{TEMP_PREFIX}{stem}-{}-{sequence}{TEMP_SUFFIX}",
        std::process::id()
    )))
}

fn remove_temporary(path: &Path) -> Result<()> {
    crate::fs_durability::remove_file(path)
        .map_err(|error| io_error("remove recovered runtime mutation replacement", error))
}

fn journal_directory(state_root: &Path) -> PathBuf {
    state_root.join("runtime-mutations")
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
