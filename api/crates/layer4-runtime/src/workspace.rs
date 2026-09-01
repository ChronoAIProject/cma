//! Versioned, hash-addressed workspace restore and capture wire.

use std::collections::BTreeSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::{Digest as _, Sha256};
use unicode_normalization::UnicodeNormalization as _;

pub const VERSION: u32 = 1;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const CAPABILITY_VERSION: u32 = 5;
pub const MAX_FILES: usize = 100_000;
pub const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_PATH_BYTES: usize = 1_024;
const MAX_HEADER_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_ARCHIVE_BYTES: usize = MAX_BYTES as usize + MAX_HEADER_BYTES + 8;
const EMPTY_CONTENT_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EntryKind {
    Regular,
    Symlink,
    Submodule,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Header {
    cma_workspace: u32,
    workspace_epoch: u64,
    #[serde(default)]
    checkpoint_version: u64,
    #[serde(default)]
    quiesced_writers: u32,
    #[serde(default, skip_serializing_if = "is_false")]
    includes_git_metadata: bool,
    source_revision: String,
    manifest_digest: String,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    path: String,
    kind: EntryKind,
    mode: u32,
    size: u64,
    content_sha256: String,
    provider_object_id: Option<String>,
    offset: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Restored {
    pub workspace_epoch: u64,
    pub source_revision: String,
    pub manifest_digest: String,
    pub entries: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct Captured {
    pub workspace_epoch: u64,
    pub checkpoint_version: u64,
    pub source_revision: String,
    pub manifest_digest: String,
    pub archive_sha256: String,
    pub entries: usize,
    pub bytes: u64,
    pub quiesced_writers: u32,
    pub includes_git_metadata: bool,
    pub archive: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct CaptureAcknowledgement<'a> {
    cma_checkpoint: u32,
    workspace_epoch: u64,
    checkpoint_version: u64,
    source_revision: &'a str,
    manifest_digest: &'a str,
    archive_sha256: &'a str,
    entries: usize,
    bytes: u64,
    quiesced_writers: u32,
    #[serde(skip_serializing_if = "is_false")]
    includes_git_metadata: bool,
    writers_resumed: bool,
    writer_state: &'static str,
    writer_resume_token: Option<&'a str>,
}

impl Captured {
    /// Length-delimited acknowledgement followed by the exact sealed archive.
    pub fn into_wire(self) -> Result<Vec<u8>, String> {
        self.into_wire_with_writer_receipt("resumed", true, None)
    }

    pub fn into_wire_held(self, resume_token: &str) -> Result<Vec<u8>, String> {
        self.into_wire_with_writer_receipt("held_for_teardown", false, Some(resume_token))
    }

    fn into_wire_with_writer_receipt(
        self,
        writer_state: &'static str,
        writers_resumed: bool,
        writer_resume_token: Option<&str>,
    ) -> Result<Vec<u8>, String> {
        let acknowledgement = serde_json::to_vec(&CaptureAcknowledgement {
            cma_checkpoint: VERSION,
            workspace_epoch: self.workspace_epoch,
            checkpoint_version: self.checkpoint_version,
            source_revision: &self.source_revision,
            manifest_digest: &self.manifest_digest,
            archive_sha256: &self.archive_sha256,
            entries: self.entries,
            bytes: self.bytes,
            quiesced_writers: self.quiesced_writers,
            includes_git_metadata: self.includes_git_metadata,
            writers_resumed,
            writer_state,
            writer_resume_token,
        })
        .map_err(|error| format!("encode checkpoint acknowledgement: {error}"))?;
        let header_len = u64::try_from(acknowledgement.len())
            .map_err(|_| "checkpoint acknowledgement is too large".to_string())?;
        let mut wire = Vec::with_capacity(
            8_usize
                .saturating_add(acknowledgement.len())
                .saturating_add(self.archive.len()),
        );
        wire.extend_from_slice(&header_len.to_be_bytes());
        wire.extend_from_slice(&acknowledgement);
        wire.extend_from_slice(&self.archive);
        Ok(wire)
    }
}

pub struct PreparedCapture {
    temporary: PathBuf,
    committed: PathBuf,
    captured: Option<Captured>,
}

impl PreparedCapture {
    /// Make a fully flushed capture authoritative after the caller has either
    /// resumed the writers or retained their fence for immediate teardown.
    pub fn commit(mut self) -> Result<Captured, String> {
        if self.committed.exists() {
            let retained = load_capture_path(
                &self.committed,
                self.captured
                    .as_ref()
                    .expect("prepared capture exists")
                    .workspace_epoch,
                self.captured
                    .as_ref()
                    .expect("prepared capture exists")
                    .checkpoint_version,
                &self
                    .captured
                    .as_ref()
                    .expect("prepared capture exists")
                    .source_revision,
                self.captured
                    .as_ref()
                    .expect("prepared capture exists")
                    .includes_git_metadata,
            )?;
            if retained.archive_sha256
                != self
                    .captured
                    .as_ref()
                    .expect("prepared capture exists")
                    .archive_sha256
            {
                return Err("checkpoint version already names different workspace bytes".into());
            }
            let _ = std::fs::remove_file(&self.temporary);
            self.temporary = PathBuf::new();
            prune_committed_captures(
                self.committed
                    .parent()
                    .ok_or_else(|| "workspace checkpoint has no storage root".to_string())?,
                retained.workspace_epoch,
                retained.checkpoint_version,
            )?;
            return Ok(retained);
        }
        std::fs::rename(&self.temporary, &self.committed)
            .map_err(|error| format!("commit workspace checkpoint: {error}"))?;
        self.temporary = PathBuf::new();
        if let Some(parent) = self.committed.parent() {
            std::fs::File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("flush workspace checkpoint directory: {error}"))?;
            let captured = self.captured.as_ref().expect("prepared capture exists");
            prune_committed_captures(
                parent,
                captured.workspace_epoch,
                captured.checkpoint_version,
            )?;
        }
        Ok(self.captured.take().expect("prepared capture exists"))
    }
}

impl Drop for PreparedCapture {
    fn drop(&mut self) {
        if !self.temporary.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}

/// Read an exact prior capture for an ambiguous retry.
pub fn load_capture(
    checkpoint_root: &Path,
    epoch: u64,
    version: u64,
    source_revision: &str,
    includes_git_metadata: bool,
) -> Result<Option<Captured>, String> {
    validate_capture_request(epoch, version, source_revision)?;
    let path = capture_path(checkpoint_root, epoch, version);
    if !path.exists() {
        if latest_capture_version(checkpoint_root, epoch)?.is_some_and(|latest| version < latest) {
            return Err(
                "workspace checkpoint version predates the retained checkpoint high-water".into(),
            );
        }
        return Ok(None);
    }
    load_capture_path(
        &path,
        epoch,
        version,
        source_revision,
        includes_git_metadata,
    )
    .map(Some)
}

/// Walk a quiescent tree, seal it deterministically, verify it, and flush a
/// temporary artifact. The caller must settle the writer disposition before
/// calling commit.
pub fn prepare_capture(
    workspace_root: &Path,
    checkpoint_root: &Path,
    epoch: u64,
    version: u64,
    source_revision: &str,
    quiesced_writers: u32,
    includes_git_metadata: bool,
) -> Result<PreparedCapture, String> {
    use std::os::unix::fs::PermissionsExt as _;

    validate_capture_request(epoch, version, source_revision)?;
    if !workspace_root.is_dir() {
        return Err("workspace root is not a directory".into());
    }
    let mut paths = Vec::new();
    collect_capture_paths(
        workspace_root,
        workspace_root,
        &mut paths,
        includes_git_metadata,
    )?;
    if paths.len() > MAX_FILES {
        return Err("workspace contains too many files".into());
    }
    paths.sort();

    let mut entries = Vec::with_capacity(paths.len());
    let mut payload = Vec::new();
    let mut offset = 0_u64;
    for relative in paths {
        validate_path(&relative)?;
        let path = workspace_root.join(&relative);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect workspace entry {relative}: {error}"))?;
        let (kind, mode, bytes) = if metadata.file_type().is_file() {
            let bytes = std::fs::read(&path)
                .map_err(|error| format!("read workspace entry {relative}: {error}"))?;
            let mode = if metadata.permissions().mode() & 0o111 == 0 {
                0o100644
            } else {
                0o100755
            };
            (EntryKind::Regular, mode, bytes)
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map_err(|error| format!("read workspace symlink {relative}: {error}"))?;
            let target = target
                .to_str()
                .ok_or_else(|| "workspace symlink target is not UTF-8".to_string())?;
            validate_symlink(&relative, target)?;
            (EntryKind::Symlink, 0o120000, target.as_bytes().to_vec())
        } else {
            return Err(format!(
                "workspace entry {relative} has an unsupported type"
            ));
        };
        let size = u64::try_from(bytes.len())
            .map_err(|_| "workspace entry exceeds this runtime".to_string())?;
        offset = offset
            .checked_add(size)
            .ok_or_else(|| "workspace size overflowed".to_string())?;
        if offset > MAX_BYTES {
            return Err("workspace exceeds its byte bound".into());
        }
        entries.push(Entry {
            path: relative,
            kind,
            mode,
            size,
            content_sha256: hex(&Sha256::digest(&bytes)),
            provider_object_id: Some(git_blob_id(&bytes)),
            offset: offset - size,
        });
        payload.extend_from_slice(&bytes);
    }

    let mut header = Header {
        cma_workspace: VERSION,
        workspace_epoch: epoch,
        checkpoint_version: version,
        quiesced_writers,
        includes_git_metadata,
        source_revision: source_revision.to_ascii_lowercase(),
        manifest_digest: "0".repeat(64),
        entries,
    };
    header.manifest_digest = manifest_digest(&header, &payload)?;
    let encoded = serde_json::to_vec(&header)
        .map_err(|error| format!("encode workspace checkpoint header: {error}"))?;
    if encoded.is_empty() || encoded.len() > MAX_HEADER_BYTES {
        return Err("workspace checkpoint header exceeds its bound".into());
    }
    let header_len = u64::try_from(encoded.len())
        .map_err(|_| "workspace checkpoint header exceeds this runtime".to_string())?;
    let mut archive = Vec::with_capacity(
        8_usize
            .saturating_add(encoded.len())
            .saturating_add(payload.len()),
    );
    archive.extend_from_slice(&header_len.to_be_bytes());
    archive.extend_from_slice(&encoded);
    archive.extend_from_slice(&payload);
    validate_archive(&archive, epoch, version, source_revision)?;

    std::fs::create_dir_all(checkpoint_root)
        .map_err(|error| format!("create workspace checkpoint directory: {error}"))?;
    let committed = capture_path(checkpoint_root, epoch, version);
    let temporary = checkpoint_root.join(format!(
        ".epoch-{epoch}-v{version}-{}-{}.pending",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("create workspace checkpoint: {error}"))?;
    file.write_all(&archive)
        .map_err(|error| format!("write workspace checkpoint: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("flush workspace checkpoint: {error}"))?;
    let captured = captured_from(header, archive);
    Ok(PreparedCapture {
        temporary,
        committed,
        captured: Some(captured),
    })
}

fn validate_capture_request(epoch: u64, version: u64, source_revision: &str) -> Result<(), String> {
    if epoch == 0 || version == 0 {
        return Err("workspace checkpoint epoch and version must be positive".into());
    }
    if source_revision.len() != 40 || !source_revision.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("workspace checkpoint source revision is invalid".into());
    }
    Ok(())
}

fn capture_path(root: &Path, epoch: u64, version: u64) -> PathBuf {
    root.join(format!("epoch-{epoch}-v{version}.pack"))
}

fn latest_capture_version(root: &Path, epoch: u64) -> Result<Option<u64>, String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect workspace checkpoint storage: {error}")),
    };
    let prefix = format!("epoch-{epoch}-v");
    let mut latest = None;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("inspect workspace checkpoint entry: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(version) = name
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(".pack"))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        latest = Some(latest.map_or(version, |current: u64| current.max(version)));
    }
    Ok(latest)
}

/// Keep the current capture and its immediate predecessor for ambiguous wire
/// retries. Older versions have already been superseded at the runtime fence
/// and would otherwise grow without bound during periodic checkpointing.
fn prune_committed_captures(root: &Path, epoch: u64, committed: u64) -> Result<(), String> {
    let entries = std::fs::read_dir(root)
        .map_err(|error| format!("inspect workspace checkpoint storage: {error}"))?;
    let prefix = format!("epoch-{epoch}-v");
    let retain_from = committed.saturating_sub(1);
    let mut removed = false;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("inspect workspace checkpoint entry: {error}"))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(version) = name
            .strip_prefix(&prefix)
            .and_then(|value| value.strip_suffix(".pack"))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        if version < retain_from {
            std::fs::remove_file(entry.path())
                .map_err(|error| format!("prune superseded workspace checkpoint: {error}"))?;
            removed = true;
        }
    }
    if removed {
        std::fs::File::open(root)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("flush pruned workspace checkpoint directory: {error}"))?;
    }
    Ok(())
}

fn load_capture_path(
    path: &Path,
    epoch: u64,
    version: u64,
    source_revision: &str,
    includes_git_metadata: bool,
) -> Result<Captured, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("inspect retained workspace checkpoint: {error}"))?;
    if metadata.len() > u64::try_from(MAX_ARCHIVE_BYTES).unwrap_or(u64::MAX) {
        return Err("retained workspace checkpoint exceeds its bound".into());
    }
    let archive = std::fs::read(path)
        .map_err(|error| format!("read retained workspace checkpoint: {error}"))?;
    let header = validate_archive(&archive, epoch, version, source_revision)?;
    if header.includes_git_metadata != includes_git_metadata {
        return Err("retained workspace checkpoint has a different capture scope".into());
    }
    Ok(captured_from(header, archive))
}

fn validate_archive(
    archive: &[u8],
    epoch: u64,
    version: u64,
    source_revision: &str,
) -> Result<Header, String> {
    if archive.len() > MAX_ARCHIVE_BYTES {
        return Err("workspace checkpoint archive exceeds its bound".into());
    }
    let (header, payload) = open(archive)?;
    if header.workspace_epoch != epoch
        || header.checkpoint_version != version
        || !header.source_revision.eq_ignore_ascii_case(source_revision)
    {
        return Err("workspace checkpoint does not match the requested fence".into());
    }
    validate_header(&header, payload)?;
    if manifest_digest(&header, payload)? != header.manifest_digest {
        return Err("workspace checkpoint manifest digest does not match its entries".into());
    }
    Ok(header)
}

fn captured_from(header: Header, archive: Vec<u8>) -> Captured {
    Captured {
        workspace_epoch: header.workspace_epoch,
        checkpoint_version: header.checkpoint_version,
        source_revision: header.source_revision,
        manifest_digest: header.manifest_digest,
        archive_sha256: hex(&Sha256::digest(&archive)),
        entries: header.entries.len(),
        bytes: header.entries.iter().map(|entry| entry.size).sum(),
        quiesced_writers: header.quiesced_writers,
        includes_git_metadata: header.includes_git_metadata,
        archive,
    }
}

fn collect_capture_paths(
    root: &Path,
    directory: &Path,
    paths: &mut Vec<String>,
    includes_git_metadata: bool,
) -> Result<(), String> {
    for item in std::fs::read_dir(directory)
        .map_err(|error| format!("inspect workspace directory: {error}"))?
    {
        let item = item.map_err(|error| format!("inspect workspace entry: {error}"))?;
        let path = item.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "workspace entry escaped its root".to_string())?
            .to_str()
            .ok_or_else(|| "workspace path is not UTF-8".to_string())?
            .to_string();
        if excluded_capture_path(&relative, includes_git_metadata) {
            continue;
        }
        validate_path(&relative)?;
        let kind = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect workspace entry {relative}: {error}"))?
            .file_type();
        if kind.is_dir() {
            collect_capture_paths(root, &path, paths, includes_git_metadata)?;
        } else if kind.is_file() || kind.is_symlink() {
            paths.push(relative);
        } else {
            return Err(format!(
                "workspace entry {relative} has an unsupported type"
            ));
        }
    }
    Ok(())
}

fn git_blob_id(bytes: &[u8]) -> String {
    let mut digest = Sha1::new();
    digest.update(format!("blob {}\0", bytes.len()).as_bytes());
    digest.update(bytes);
    hex(&digest.finalize())
}

/// Verify a complete archive, materialize it away from the live path, verify
/// the resulting filesystem, and only then make it visible.
pub fn restore(
    target: &Path,
    archive: &[u8],
    expected_epoch: u64,
    expected_source: &str,
    expected_manifest: &str,
    expected_archive_sha256: &str,
) -> Result<Restored, String> {
    require_sha256(expected_archive_sha256, "archive")?;
    if hex(&Sha256::digest(archive)) != expected_archive_sha256 {
        return Err("workspace archive failed its expected SHA-256".into());
    }
    let (header, payload) = open(archive)?;
    if header.workspace_epoch != expected_epoch
        || header.source_revision != expected_source
        || header.manifest_digest != expected_manifest
    {
        return Err("workspace archive does not match the requested epoch and baseline".into());
    }
    validate_header(&header, payload)?;
    if manifest_digest(&header, payload)? != header.manifest_digest {
        return Err("workspace manifest digest does not match its entries".into());
    }

    if target.exists() {
        let mut existing = std::fs::read_dir(target)
            .map_err(|error| format!("inspect existing workspace: {error}"))?;
        let empty = existing
            .next()
            .transpose()
            .map_err(|error| error.to_string())?
            .is_none();
        if !empty {
            verify_existing_tree(target, &header.entries, payload).map_err(|why| {
                format!("the existing workspace does not match this archive: {why}")
            })?;
            return Ok(restored(&header));
        }
        std::fs::remove_dir(target)
            .map_err(|error| format!("remove empty workspace target: {error}"))?;
    }

    let parent = target
        .parent()
        .ok_or_else(|| "workspace target has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| format!("create workspace parent: {error}"))?;
    let staging = staging_path(target, expected_epoch);
    std::fs::create_dir(&staging)
        .map_err(|error| format!("create workspace staging directory: {error}"))?;
    let mut cleanup = Staging(Some(staging.clone()));
    materialize(&staging, &header.entries, payload)?;
    verify_materialized(&staging, &header.entries, payload)?;

    std::fs::rename(&staging, target)
        .map_err(|error| format!("commit restored workspace: {error}"))?;
    cleanup.0 = None;
    Ok(restored(&header))
}

fn restored(header: &Header) -> Restored {
    let bytes = header.entries.iter().map(|entry| entry.size).sum();
    Restored {
        workspace_epoch: header.workspace_epoch,
        source_revision: header.source_revision.clone(),
        manifest_digest: header.manifest_digest.clone(),
        entries: header.entries.len(),
        bytes,
    }
}

fn open(archive: &[u8]) -> Result<(Header, &[u8]), String> {
    let prefix: [u8; 8] = archive
        .get(..8)
        .ok_or_else(|| "workspace archive has no header length".to_string())?
        .try_into()
        .map_err(|_| "workspace archive header length is malformed".to_string())?;
    let header_len = usize::try_from(u64::from_be_bytes(prefix))
        .map_err(|_| "workspace archive header exceeds this runtime".to_string())?;
    if header_len == 0 || header_len > MAX_HEADER_BYTES {
        return Err("workspace archive header is outside its bound".into());
    }
    let header_end = 8_usize
        .checked_add(header_len)
        .ok_or_else(|| "workspace archive header length overflowed".to_string())?;
    let header: Header = serde_json::from_slice(
        archive
            .get(8..header_end)
            .ok_or_else(|| "workspace archive header is truncated".to_string())?,
    )
    .map_err(|error| format!("workspace archive header is invalid: {error}"))?;
    Ok((header, &archive[header_end..]))
}

fn validate_header(header: &Header, payload: &[u8]) -> Result<(), String> {
    if header.cma_workspace != VERSION || header.workspace_epoch == 0 {
        return Err("workspace archive version or epoch is unsupported".into());
    }
    if header.entries.len() > MAX_FILES {
        return Err("workspace archive contains too many files".into());
    }
    require_sha256(&header.manifest_digest, "manifest")?;
    if header.source_revision.len() != 40
        || !header
            .source_revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("workspace archive source revision is invalid".into());
    }
    let mut seen_aliases = BTreeSet::new();
    let mut previous_path: Option<&str> = None;
    let mut expected_offset = 0_u64;
    for entry in &header.entries {
        if excluded_capture_path(&entry.path, header.includes_git_metadata) {
            return Err("workspace archive contains a path outside its capture scope".into());
        }
        let alias = workspace_path_alias(&entry.path)?;
        if previous_path.is_some_and(|previous| {
            previous >= entry.path.as_str()
                || entry
                    .path
                    .strip_prefix(previous)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        }) || !seen_aliases.insert(alias)
        {
            return Err("workspace archive paths are repeated, nested, or out of order".into());
        }
        previous_path = Some(&entry.path);
        if entry.offset != expected_offset {
            return Err("workspace archive entries overlap or contain a gap".into());
        }
        expected_offset = expected_offset
            .checked_add(entry.size)
            .ok_or_else(|| "workspace archive size overflowed".to_string())?;
        if expected_offset > MAX_BYTES {
            return Err("workspace archive exceeds its byte bound".into());
        }
        require_sha256(&entry.content_sha256, "entry")?;
        match entry.kind {
            EntryKind::Regular if matches!(entry.mode, 0o100644 | 0o100755) => {}
            EntryKind::Symlink if entry.mode == 0o120000 => {}
            EntryKind::Submodule
                if entry.mode == 0o160000
                    && entry.size == 0
                    && entry.content_sha256 == EMPTY_CONTENT_SHA256
                    && entry.provider_object_id.is_some() => {}
            _ => return Err("workspace archive contains an unsupported file mode".into()),
        }
        if let Some(object) = entry.provider_object_id.as_deref() {
            if object.len() != 40 || !object.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err("workspace archive contains an invalid Git object ID".into());
            }
        }
        let bytes = entry_bytes(entry, payload)?;
        if hex(&Sha256::digest(bytes)) != entry.content_sha256 {
            return Err(format!("workspace entry {} failed its SHA-256", entry.path));
        }
        if entry.kind == EntryKind::Symlink {
            validate_symlink(
                &entry.path,
                std::str::from_utf8(bytes)
                    .map_err(|_| "workspace symlink target is not UTF-8".to_string())?,
            )?;
        }
    }
    if seen_aliases.iter().any(|alias| {
        let mut parent = alias.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            if seen_aliases.contains(prefix) {
                return true;
            }
            parent = prefix;
        }
        false
    }) {
        return Err("workspace archive paths alias a file as a directory".into());
    }
    if usize::try_from(expected_offset).ok() != Some(payload.len()) {
        return Err("workspace archive payload has trailing or missing bytes".into());
    }
    Ok(())
}

fn materialize(root: &Path, entries: &[Entry], payload: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::{symlink, PermissionsExt as _};

    for entry in entries {
        let destination = root.join(&entry.path);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("create workspace directory: {error}"))?;
        }
        let bytes = entry_bytes(entry, payload)?;
        match entry.kind {
            EntryKind::Regular => {
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .map_err(|error| format!("create workspace file {}: {error}", entry.path))?;
                file.write_all(bytes)
                    .map_err(|error| format!("write workspace file {}: {error}", entry.path))?;
                file.sync_all()
                    .map_err(|error| format!("flush workspace file {}: {error}", entry.path))?;
                std::fs::set_permissions(
                    &destination,
                    std::fs::Permissions::from_mode(entry.mode & 0o777),
                )
                .map_err(|error| format!("set workspace mode {}: {error}", entry.path))?;
            }
            EntryKind::Symlink => {
                let target = std::str::from_utf8(bytes)
                    .map_err(|_| "workspace symlink target is not UTF-8".to_string())?;
                symlink(target, &destination)
                    .map_err(|error| format!("create workspace symlink {}: {error}", entry.path))?;
            }
            EntryKind::Submodule => std::fs::create_dir(&destination).map_err(|error| {
                format!("create inert submodule directory {}: {error}", entry.path)
            })?,
        }
    }
    Ok(())
}

fn verify_materialized(root: &Path, entries: &[Entry], payload: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;

    for entry in entries {
        let path = root.join(&entry.path);
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("verify workspace entry {}: {error}", entry.path))?;
        match entry.kind {
            EntryKind::Regular => {
                if !metadata.file_type().is_file()
                    || metadata.permissions().mode() & 0o777 != entry.mode & 0o777
                    || std::fs::read(&path)
                        .map_err(|error| format!("read workspace entry {}: {error}", entry.path))?
                        != entry_bytes(entry, payload)?
                {
                    return Err(format!(
                        "materialized workspace file {} disagrees",
                        entry.path
                    ));
                }
            }
            EntryKind::Symlink => {
                let expected_target = std::str::from_utf8(entry_bytes(entry, payload)?)
                    .map_err(|_| "workspace symlink target is not UTF-8")?;
                if !metadata.file_type().is_symlink()
                    || std::fs::read_link(&path)
                        .map_err(|error| format!("read workspace link {}: {error}", entry.path))?
                        != Path::new(expected_target)
                {
                    return Err(format!(
                        "materialized workspace link {} disagrees",
                        entry.path
                    ));
                }
            }
            EntryKind::Submodule => {
                let empty = metadata.file_type().is_dir()
                    && std::fs::read_dir(&path)
                        .map_err(|error| {
                            format!("inspect submodule directory {}: {error}", entry.path)
                        })?
                        .next()
                        .transpose()
                        .map_err(|error| {
                            format!("inspect submodule directory {}: {error}", entry.path)
                        })?
                        .is_none();
                if !empty {
                    return Err(format!(
                        "materialized submodule directory {} is not inert",
                        entry.path
                    ));
                }
            }
        }
    }
    Ok(())
}

fn verify_existing_tree(root: &Path, entries: &[Entry], payload: &[u8]) -> Result<(), String> {
    let expected_files = entries
        .iter()
        .filter(|entry| entry.kind != EntryKind::Submodule)
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    let mut expected_directories = BTreeSet::new();
    for entry in entries {
        if entry.kind == EntryKind::Submodule {
            expected_directories.insert(entry.path.clone());
        }
        let mut parent = Path::new(&entry.path).parent();
        while let Some(path) = parent {
            if path.as_os_str().is_empty() {
                break;
            }
            expected_directories.insert(
                path.to_str()
                    .ok_or_else(|| "workspace path is not UTF-8".to_string())?
                    .to_string(),
            );
            parent = path.parent();
        }
    }

    let mut actual_files = BTreeSet::new();
    let mut actual_directories = BTreeSet::new();
    collect_materialized_paths(root, root, &mut actual_files, &mut actual_directories)?;
    if actual_files != expected_files || actual_directories != expected_directories {
        return Err("its path set differs".into());
    }
    verify_materialized(root, entries, payload)
}

fn collect_materialized_paths(
    root: &Path,
    directory: &Path,
    files: &mut BTreeSet<String>,
    directories: &mut BTreeSet<String>,
) -> Result<(), String> {
    for item in std::fs::read_dir(directory)
        .map_err(|error| format!("inspect workspace directory: {error}"))?
    {
        let item = item.map_err(|error| format!("inspect workspace entry: {error}"))?;
        let path = item.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| "workspace entry escaped its root".to_string())?
            .to_str()
            .ok_or_else(|| "workspace path is not UTF-8".to_string())?
            .to_string();
        validate_path(&relative)?;
        let kind = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("inspect workspace entry {relative}: {error}"))?
            .file_type();
        if kind.is_dir() {
            directories.insert(relative);
            collect_materialized_paths(root, &path, files, directories)?;
        } else if kind.is_file() || kind.is_symlink() {
            files.insert(relative);
        } else {
            return Err("it contains an unsupported filesystem entry".into());
        }
    }
    Ok(())
}

fn entry_bytes<'a>(entry: &Entry, payload: &'a [u8]) -> Result<&'a [u8], String> {
    let start = usize::try_from(entry.offset)
        .map_err(|_| "workspace entry offset exceeds this runtime".to_string())?;
    let size = usize::try_from(entry.size)
        .map_err(|_| "workspace entry size exceeds this runtime".to_string())?;
    let end = start
        .checked_add(size)
        .ok_or_else(|| "workspace entry range overflowed".to_string())?;
    payload
        .get(start..end)
        .ok_or_else(|| "workspace entry extends past the payload".to_string())
}

fn manifest_digest(header: &Header, payload: &[u8]) -> Result<String, String> {
    let mut digest = Sha256::new();
    digest.update(b"cma-workspace-manifest-v1\0");
    if header.includes_git_metadata {
        digest.update(b"complete-git-workspace-v1\0");
    }
    hash_text(&mut digest, &header.source_revision)?;
    digest.update(
        u64::try_from(header.entries.len())
            .map_err(|_| "workspace manifest is too large".to_string())?
            .to_be_bytes(),
    );
    for entry in &header.entries {
        hash_text(&mut digest, &entry.path)?;
        digest.update([match entry.kind {
            EntryKind::Regular => 1,
            EntryKind::Symlink => 2,
            EntryKind::Submodule => 3,
        }]);
        digest.update(entry.mode.to_be_bytes());
        digest.update(entry.size.to_be_bytes());
        hash_text(&mut digest, &entry.content_sha256)?;
        hash_optional_text(&mut digest, entry.provider_object_id.as_deref())?;
        let target = if entry.kind == EntryKind::Symlink {
            Some(
                std::str::from_utf8(entry_bytes(entry, payload)?)
                    .map_err(|_| "workspace symlink target is not UTF-8".to_string())?,
            )
        } else {
            None
        };
        hash_optional_text(&mut digest, target)?;
    }
    Ok(hex(&digest.finalize()))
}

fn hash_text(digest: &mut Sha256, value: &str) -> Result<(), String> {
    digest.update(
        u64::try_from(value.len())
            .map_err(|_| "workspace manifest value is too large".to_string())?
            .to_be_bytes(),
    );
    digest.update(value.as_bytes());
    Ok(())
}

fn hash_optional_text(digest: &mut Sha256, value: Option<&str>) -> Result<(), String> {
    match value {
        Some(value) => {
            digest.update([1]);
            hash_text(digest, value)
        }
        None => {
            digest.update([0]);
            Ok(())
        }
    }
}

fn validate_path(path: &str) -> Result<(), String> {
    workspace_path_alias(path).map(|_| ())
}

fn workspace_path_alias(path: &str) -> Result<String, String> {
    if path.is_empty()
        || path.len() > MAX_PATH_BYTES
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains('\0')
    {
        return Err("workspace archive contains a non-canonical path".into());
    }
    let mut aliases = Vec::new();
    for segment in path.split('/') {
        let alias = segment
            .nfkc()
            .flat_map(char::to_lowercase)
            .collect::<String>();
        if segment.is_empty()
            || segment.chars().any(unsafe_path_character)
            || alias.is_empty()
            || matches!(alias.as_str(), "." | "..")
            || alias
                .chars()
                .any(|character| matches!(character, '/' | '\\' | ':'))
            || alias.ends_with('.')
            || alias.ends_with(' ')
            || excluded_capture_segment(&alias, true)
            || reserved_device_segment(&alias)
        {
            return Err("workspace archive contains a non-canonical path".into());
        }
        aliases.push(alias);
    }
    Ok(aliases.join("/"))
}

fn unsafe_path_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200e}'
                | '\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2066}'..='\u{2069}'
        )
}

fn reserved_device_segment(segment: &str) -> bool {
    let stem = segment.split('.').next().unwrap_or(segment);
    matches!(stem, "con" | "prn" | "aux" | "nul" | "clock$")
        || stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

fn excluded_capture_path(path: &str, includes_git_metadata: bool) -> bool {
    path.split('/')
        .any(|segment| excluded_capture_segment(segment, includes_git_metadata))
}

/// Files owned by the runtime or conventionally used as credential material
/// are outside the repository durability boundary. Matching every path segment
/// also prevents a nested tool directory from becoming checkpoint content.
fn excluded_capture_segment(segment: &str, includes_git_metadata: bool) -> bool {
    let lower = segment.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        ".cma"
            | ".chrono"
            | ".codex"
            | ".claude"
            | ".nyxid"
            | ".ssh"
            | ".aws"
            | ".gnupg"
            | ".netrc"
            | ".npmrc"
            | ".pypirc"
            | "credentials"
            | "credentials.json"
            | "id_rsa"
            | "id_ed25519"
    ) || (!includes_git_metadata && lower == ".git")
        || lower == ".env"
        || lower.starts_with(".env.")
        || lower.starts_with(".cma-")
        || lower.starts_with(".chrono-")
}

fn is_false(value: &bool) -> bool {
    !*value
}

fn validate_symlink(path: &str, target: &str) -> Result<(), String> {
    if target.is_empty()
        || target.len() > MAX_PATH_BYTES
        || target.starts_with('/')
        || target.contains('\\')
        || target.contains('\0')
    {
        return Err("workspace archive contains an unsafe symlink".into());
    }
    let mut depth = path.split('/').count().saturating_sub(1) as i64;
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                depth -= 1;
                if depth < 0 {
                    return Err("workspace symlink escapes its root".into());
                }
            }
            _ => depth += 1,
        }
    }
    Ok(())
}

fn require_sha256(value: &str, name: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("workspace {name} SHA-256 is invalid"));
    }
    Ok(())
}

fn staging_path(target: &Path, epoch: u64) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    target.with_file_name(format!(
        ".cma-workspace-{}-{epoch}-{stamp}",
        std::process::id()
    ))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

struct Staging(Option<PathBuf>);

impl Drop for Staging {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod tests;
