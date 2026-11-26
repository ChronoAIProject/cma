use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::command::hex;
use crate::failure::{Result, RuntimeError};
use crate::paths::{lexical_target, safe_relative};

pub(crate) const FORMAT_VERSION: u32 = 1;
pub(crate) const MAX_ENTRIES: usize = 500_000;
pub(crate) const MAX_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const RUNTIME_TRANSIENT_PATHS: [&str; 2] = [".cma/preparing", ".cma/removing"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub version: u32,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManifestEntry {
    pub path: String,
    pub kind: EntryKind,
    pub mode: u32,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

pub(crate) fn collect(root: &Path, exclusions: &[String]) -> Result<Manifest> {
    let exclusions = validate_exclusions(exclusions)?;
    let root_metadata =
        std::fs::symlink_metadata(root).map_err(manifest_io("inspect home root"))?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(RuntimeError::rejected(
            "checkpoint_home_invalid",
            "sandbox home is not a directory",
        ));
    }
    let mut pending = vec![root.to_path_buf()];
    let mut entries = Vec::new();
    let mut payload_bytes = 0_u64;
    while let Some(directory) = pending.pop() {
        let mut children = std::fs::read_dir(&directory)
            .map_err(manifest_io("list checkpoint directory"))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(manifest_io("read checkpoint directory entry"))?;
        children.sort_by_key(std::fs::DirEntry::file_name);
        for child in children {
            let path = child.path();
            let relative = path
                .strip_prefix(root)
                .expect("checkpoint entry remains below root");
            if runtime_transient(relative) || excluded(relative, &exclusions) {
                continue;
            }
            if entries.len() >= MAX_ENTRIES {
                return Err(RuntimeError::rejected(
                    "checkpoint_too_large",
                    format!("checkpoint contains more than {MAX_ENTRIES} entries"),
                ));
            }
            let path_text = relative
                .to_str()
                .ok_or_else(|| {
                    RuntimeError::rejected(
                        "checkpoint_path_invalid",
                        "checkpoint path is not UTF-8",
                    )
                })?
                .to_string();
            safe_relative(&path_text, "checkpoint path")?;
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(manifest_io("inspect checkpoint entry"))?;
            let mode = file_mode(&metadata);
            let kind = metadata.file_type();
            if kind.is_dir() {
                entries.push(directory_entry(path_text, mode));
                pending.push(path);
            } else if kind.is_file() {
                payload_bytes = payload_bytes
                    .checked_add(metadata.len())
                    .ok_or_else(|| invalid("checkpoint payload overflows"))?;
                if payload_bytes > MAX_PAYLOAD_BYTES {
                    return Err(RuntimeError::rejected(
                        "checkpoint_too_large",
                        format!("checkpoint payload exceeds {MAX_PAYLOAD_BYTES} bytes"),
                    ));
                }
                entries.push(file_entry(path_text, mode, &path, metadata.len())?);
            } else if kind.is_symlink() {
                let target =
                    std::fs::read_link(&path).map_err(manifest_io("read checkpoint symlink"))?;
                if lexical_target(root, &path, &target).is_none() {
                    return Err(RuntimeError::rejected(
                        "checkpoint_symlink_escape",
                        format!("checkpoint symlink {path_text} escapes the sandbox home"),
                    ));
                }
                let target = target.to_str().ok_or_else(|| {
                    RuntimeError::rejected(
                        "checkpoint_symlink_invalid",
                        "symlink target is not UTF-8",
                    )
                })?;
                entries.push(symlink_entry(path_text, mode, target));
            } else {
                return Err(RuntimeError::rejected(
                    "checkpoint_entry_unsupported",
                    format!("checkpoint path {path_text} is not a file, directory, or symlink"),
                ));
            }
        }
    }
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    assign_offsets(&mut entries)?;
    Ok(Manifest {
        version: FORMAT_VERSION,
        entries,
    })
}

pub(crate) fn validate(manifest: &Manifest, root: &Path, payload_bytes: u64) -> Result<()> {
    if manifest.version != FORMAT_VERSION || manifest.entries.len() > MAX_ENTRIES {
        return Err(invalid(
            "checkpoint manifest version or entry count is invalid",
        ));
    }
    let mut prior: Option<&str> = None;
    let mut known = std::collections::BTreeMap::<PathBuf, EntryKind>::new();
    let mut next_offset = 0_u64;
    for entry in &manifest.entries {
        if prior.is_some_and(|prior| prior >= entry.path.as_str()) {
            return Err(invalid(
                "checkpoint manifest paths are not strictly ordered",
            ));
        }
        prior = Some(&entry.path);
        let relative = safe_relative(&entry.path, "checkpoint path")?;
        if runtime_transient(&relative) {
            return Err(invalid(
                "checkpoint contains runtime-owned workspace transition state",
            ));
        }
        if entry.mode > 0o777 {
            return Err(invalid(
                "checkpoint entry mode is outside the portable permission mask",
            ));
        }
        if let Some(parent) = relative
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            if known.get(parent) != Some(&EntryKind::Directory) {
                return Err(invalid(
                    "checkpoint entry parent is not a declared directory",
                ));
            }
        }
        validate_entry(entry, root, &relative, &mut next_offset)?;
        known.insert(relative, entry.kind);
    }
    if next_offset != payload_bytes || next_offset > MAX_PAYLOAD_BYTES {
        return Err(invalid(
            "checkpoint payload length does not match its manifest",
        ));
    }
    Ok(())
}

fn validate_entry(
    entry: &ManifestEntry,
    root: &Path,
    relative: &Path,
    offset: &mut u64,
) -> Result<()> {
    match entry.kind {
        EntryKind::Directory => {
            if entry.size != 0
                || entry.sha256.is_some()
                || entry.target.is_some()
                || entry.offset.is_some()
            {
                return Err(invalid("checkpoint directory carries file-only fields"));
            }
        }
        EntryKind::File => {
            let digest = entry
                .sha256
                .as_deref()
                .ok_or_else(|| invalid("checkpoint file has no digest"))?;
            require_sha256(digest)?;
            if entry.target.is_some() || entry.offset != Some(*offset) {
                return Err(invalid("checkpoint file offset or target is invalid"));
            }
            *offset = offset
                .checked_add(entry.size)
                .ok_or_else(|| invalid("checkpoint payload overflows"))?;
        }
        EntryKind::Symlink => {
            let target = entry
                .target
                .as_deref()
                .ok_or_else(|| invalid("checkpoint symlink has no target"))?;
            if entry.size != 0
                || entry.sha256.is_some()
                || entry.offset.is_some()
                || target.contains('\0')
            {
                return Err(invalid("checkpoint symlink carries invalid fields"));
            }
            if lexical_target(root, &root.join(relative), Path::new(target)).is_none() {
                return Err(invalid("checkpoint symlink escapes the sandbox home"));
            }
        }
    }
    Ok(())
}

fn validate_exclusions(exclusions: &[String]) -> Result<Vec<PathBuf>> {
    let effective = layer3_adapter::codex::checkpoint_archive_exclusions(exclusions);
    let mut paths = effective
        .iter()
        .map(|path| safe_relative(path, "checkpoint exclusion"))
        .collect::<Result<Vec<_>>>()?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn excluded(path: &Path, exclusions: &[PathBuf]) -> bool {
    exclusions
        .iter()
        .any(|excluded| path == excluded || path.starts_with(excluded))
}

fn runtime_transient(path: &Path) -> bool {
    RUNTIME_TRANSIENT_PATHS.iter().any(|transient| {
        let transient = Path::new(transient);
        path == transient || path.starts_with(transient)
    })
}

fn directory_entry(path: String, mode: u32) -> ManifestEntry {
    ManifestEntry {
        path,
        kind: EntryKind::Directory,
        mode,
        size: 0,
        sha256: None,
        target: None,
        offset: None,
    }
}

fn file_entry(path: String, mode: u32, source: &Path, size: u64) -> Result<ManifestEntry> {
    Ok(ManifestEntry {
        path,
        kind: EntryKind::File,
        mode,
        size,
        sha256: Some(sha256_file(source)?),
        target: None,
        offset: None,
    })
}

fn symlink_entry(path: String, mode: u32, target: &str) -> ManifestEntry {
    ManifestEntry {
        path,
        kind: EntryKind::Symlink,
        mode,
        size: 0,
        sha256: None,
        target: Some(target.to_string()),
        offset: None,
    }
}

fn assign_offsets(entries: &mut [ManifestEntry]) -> Result<()> {
    let mut offset = 0_u64;
    for entry in entries
        .iter_mut()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        entry.offset = Some(offset);
        offset = offset
            .checked_add(entry.size)
            .ok_or_else(|| invalid("checkpoint payload overflows"))?;
        if offset > MAX_PAYLOAD_BYTES {
            return Err(RuntimeError::rejected(
                "checkpoint_too_large",
                format!("checkpoint payload exceeds {MAX_PAYLOAD_BYTES} bytes"),
            ));
        }
    }
    Ok(())
}

pub(crate) fn sha256_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(manifest_io("open checkpoint file"))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(manifest_io("read checkpoint file"))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex(&digest.finalize()))
}

pub(crate) fn require_sha256(value: &str) -> Result<()> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(invalid(
            "checkpoint SHA-256 is not 64 hexadecimal characters",
        ))
    }
}

#[cfg(unix)]
fn file_mode(metadata: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt as _;
    metadata.mode() & 0o777
}

#[cfg(not(unix))]
fn file_mode(_metadata: &std::fs::Metadata) -> u32 {
    0o644
}

fn invalid(detail: &'static str) -> RuntimeError {
    RuntimeError::rejected("checkpoint_invalid", detail)
}

fn manifest_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| RuntimeError::unavailable("checkpoint_io_failed", format!("{operation}: {error}"))
}

#[cfg(test)]
#[path = "checkpoint_manifest_tests.rs"]
mod tests;
