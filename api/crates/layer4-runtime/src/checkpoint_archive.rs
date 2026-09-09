use std::io::{BufWriter, Read as _, Write as _};
use std::path::{Path, PathBuf};

use flate2::{write::GzEncoder, Compression};
use sha2::{Digest, Sha256};

use crate::checkpoint_manifest::{EntryKind, Manifest, ManifestEntry};
use crate::command::hex;
use crate::failure::{Result, RuntimeError};

mod reader;

const MAGIC: &[u8; 16] = b"CMAHOMEARCHIVE1\n";
const HEADER_BYTES: u64 = 24;
const MAX_MANIFEST_BYTES: u64 = 128 * 1024 * 1024;
pub(crate) const MAX_ARCHIVE_BYTES: u64 =
    HEADER_BYTES + MAX_MANIFEST_BYTES + crate::checkpoint_manifest::MAX_PAYLOAD_BYTES;

pub(crate) struct SealedArchive {
    pub reference: String,
    pub sha256: String,
    pub path: PathBuf,
}

#[derive(Debug)]
pub(crate) struct InspectedArchive {
    pub manifest: Manifest,
    payload_bytes: u64,
}

pub(crate) fn seal(root: &Path, store: &Path, exclusions: &[String]) -> Result<SealedArchive> {
    ensure_store(store)?;
    let manifest = crate::checkpoint_manifest::collect(root, exclusions)?;
    let encoded = serde_json::to_vec(&manifest).map_err(|error| {
        RuntimeError::unavailable(
            "checkpoint_manifest_failed",
            format!("encode checkpoint manifest: {error}"),
        )
    })?;
    if encoded.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(RuntimeError::rejected(
            "checkpoint_too_large",
            "checkpoint manifest exceeds its size limit",
        ));
    }
    let temporary = store.join(format!(".capture-{}-{}.tmp", std::process::id(), stamp()));
    let mut cleanup = Cleanup::new(temporary.clone());
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(archive_io("create checkpoint archive"))?;
    let output = ArchiveWriter {
        file: BufWriter::new(file),
        size: 0,
    };
    let mut archive = GzEncoder::new(output, Compression::fast());
    archive
        .write_all(MAGIC)
        .and_then(|_| archive.write_all(&(encoded.len() as u64).to_be_bytes()))
        .and_then(|_| archive.write_all(&encoded))
        .map_err(archive_io("write checkpoint header"))?;
    for entry in manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::File)
    {
        copy_source(root, entry, &mut archive)?;
    }
    let mut archive = archive
        .finish()
        .map_err(archive_io("finish compressed checkpoint archive"))?;
    archive
        .flush()
        .map_err(archive_io("flush checkpoint archive"))?;
    archive
        .file
        .get_ref()
        .sync_all()
        .map_err(archive_io("sync checkpoint archive"))?;
    drop(archive);

    let sha256 = crate::checkpoint_manifest::sha256_file(&temporary)?;
    let reference = format!("home-{sha256}.cma");
    let path = store.join(&reference);
    match crate::fs_durability::hard_link(&temporary, &path) {
        Ok(()) => {
            crate::fs_durability::remove_file(&temporary)
                .map_err(archive_io("remove linked checkpoint temporary"))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if !regular_file(&path) || crate::checkpoint_manifest::sha256_file(&path)? != sha256 {
                return Err(RuntimeError::rejected(
                    "checkpoint_collision",
                    "checkpoint object name is occupied by different bytes",
                ));
            }
            crate::fs_durability::remove_file(&temporary)
                .map_err(archive_io("remove duplicate checkpoint"))?;
        }
        Err(error) => return Err(archive_io("publish checkpoint archive")(error)),
    }
    cleanup.disarm();
    Ok(SealedArchive {
        reference,
        sha256,
        path,
    })
}

pub(crate) fn inspect(path: &Path, expected_sha256: &str, home: &Path) -> Result<InspectedArchive> {
    let (inspected, mut archive) = open_validated_manifest(path, expected_sha256, home)?;
    if archive.is_compressed() {
        archive.verify_payload(inspected.payload_bytes)?;
    }
    Ok(inspected)
}

fn open_validated_manifest(
    path: &Path,
    expected_sha256: &str,
    home: &Path,
) -> Result<(InspectedArchive, reader::ArchiveReader)> {
    crate::checkpoint_manifest::require_sha256(expected_sha256)?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(archive_io("inspect checkpoint archive"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("checkpoint archive is not a regular file"));
    }
    let length = metadata.len();
    if length > MAX_ARCHIVE_BYTES {
        return Err(invalid("checkpoint archive exceeds its size limit"));
    }
    let actual = crate::checkpoint_manifest::sha256_file(path)?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(RuntimeError::rejected(
            "checkpoint_checksum_mismatch",
            "checkpoint archive did not match its sealed SHA-256",
        ));
    }
    let mut archive = reader::ArchiveReader::open(path)?;
    if !archive.is_compressed() && length < HEADER_BYTES {
        return Err(invalid("checkpoint archive is truncated"));
    }
    let mut magic = [0_u8; 16];
    let mut manifest_length = [0_u8; 8];
    archive
        .read_exact(&mut magic)
        .and_then(|_| archive.read_exact(&mut manifest_length))
        .map_err(|_| invalid("checkpoint archive is truncated"))?;
    if &magic != MAGIC {
        return Err(invalid("checkpoint archive magic is invalid"));
    }
    let manifest_length = u64::from_be_bytes(manifest_length);
    if manifest_length == 0 || manifest_length > MAX_MANIFEST_BYTES {
        return Err(invalid("checkpoint manifest length is invalid"));
    }
    let payload_start = HEADER_BYTES
        .checked_add(manifest_length)
        .ok_or_else(|| invalid("checkpoint manifest length overflows"))?;
    if !archive.is_compressed() && payload_start > length {
        return Err(invalid(
            "checkpoint archive is truncated before its payload",
        ));
    }
    let mut encoded = vec![0_u8; manifest_length as usize];
    archive
        .read_exact(&mut encoded)
        .map_err(|_| invalid("checkpoint manifest is truncated"))?;
    let manifest: Manifest = serde_json::from_slice(&encoded).map_err(|error| {
        RuntimeError::rejected(
            "checkpoint_invalid",
            format!("decode checkpoint manifest: {error}"),
        )
    })?;
    let payload_bytes = if archive.is_compressed() {
        manifest.entries.iter().try_fold(0_u64, |size, entry| {
            size.checked_add(entry.size)
                .filter(|size| *size <= crate::checkpoint_manifest::MAX_PAYLOAD_BYTES)
                .ok_or_else(|| invalid("checkpoint payload exceeds its size limit"))
        })?
    } else {
        length - payload_start
    };
    crate::checkpoint_manifest::validate(&manifest, home, payload_bytes)?;
    Ok((
        InspectedArchive {
            manifest,
            payload_bytes,
        },
        archive,
    ))
}

pub(crate) fn extract(
    archive_path: &Path,
    expected_sha256: &str,
    home: &Path,
    destination: &Path,
) -> Result<()> {
    // Validate identity and every path before staging. Verify file digests and the
    // compression trailer while extracting, before the caller may replace home.
    // Continuing this reader avoids decompressing the entire payload twice.
    let (inspected, mut archive) = open_validated_manifest(archive_path, expected_sha256, home)?;
    std::fs::create_dir(destination).map_err(archive_io("create restore staging directory"))?;
    let mut directories = Vec::new();
    for entry in &inspected.manifest.entries {
        let path = destination.join(&entry.path);
        match entry.kind {
            EntryKind::Directory => {
                std::fs::create_dir(&path).map_err(archive_io("create restored directory"))?;
                directories.push((path, entry.mode));
            }
            EntryKind::File => extract_file(&mut archive, entry, &path)?,
            EntryKind::Symlink => {}
        }
    }
    archive.finish()?;
    for entry in inspected
        .manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == EntryKind::Symlink)
    {
        let target = entry
            .target
            .as_deref()
            .expect("validated symlink has a target");
        create_symlink(Path::new(target), &destination.join(&entry.path))
            .map_err(archive_io("create restored symlink"))?;
    }
    directories.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, mode) in directories {
        set_mode(&path, mode)?;
    }
    Ok(())
}

fn copy_source(root: &Path, entry: &ManifestEntry, output: &mut impl std::io::Write) -> Result<()> {
    let path = root.join(&entry.path);
    let metadata =
        std::fs::symlink_metadata(&path).map_err(archive_io("recheck checkpoint file"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != entry.size {
        return Err(RuntimeError::rejected(
            "checkpoint_changed",
            format!("checkpoint file {} changed during capture", entry.path),
        ));
    }
    let mut source =
        std::fs::File::open(&path).map_err(archive_io("open checkpoint payload file"))?;
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(archive_io("read checkpoint payload"))?;
        if read == 0 {
            break;
        }
        if copied.saturating_add(read as u64) > entry.size {
            return Err(RuntimeError::rejected(
                "checkpoint_changed",
                "checkpoint file grew during capture",
            ));
        }
        output
            .write_all(&buffer[..read])
            .map_err(archive_io("write checkpoint payload"))?;
        digest.update(&buffer[..read]);
        copied += read as u64;
    }
    if copied != entry.size || entry.sha256.as_deref() != Some(hex(&digest.finalize()).as_str()) {
        return Err(RuntimeError::rejected(
            "checkpoint_changed",
            format!("checkpoint file {} changed during capture", entry.path),
        ));
    }
    Ok(())
}

fn extract_file(
    archive: &mut impl std::io::Read,
    entry: &ManifestEntry,
    destination: &Path,
) -> Result<()> {
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(archive_io("create restored file"))?;
    let mut limited = archive.take(entry.size);
    let mut digest = Sha256::new();
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = limited
            .read(&mut buffer)
            .map_err(archive_io("read restored file"))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(archive_io("write restored file"))?;
        digest.update(&buffer[..read]);
        copied += read as u64;
    }
    let actual = hex(&digest.finalize());
    if copied != entry.size || entry.sha256.as_deref() != Some(actual.as_str()) {
        return Err(invalid(
            "checkpoint file payload does not match its manifest",
        ));
    }
    output.flush().map_err(archive_io("flush restored file"))?;
    set_mode(destination, entry.mode)
}

struct ArchiveWriter {
    file: BufWriter<std::fs::File>,
    size: u64,
}

impl std::io::Write for ArchiveWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.size.saturating_add(bytes.len() as u64) > MAX_ARCHIVE_BYTES {
            return Err(std::io::Error::other(
                "compressed checkpoint exceeds its size limit",
            ));
        }
        let written = self.file.write(bytes)?;
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn ensure_store(store: &Path) -> Result<()> {
    crate::fs_durability::create_dir_all(store).map_err(archive_io("create checkpoint store"))?;
    let metadata =
        std::fs::symlink_metadata(store).map_err(archive_io("inspect checkpoint store"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(RuntimeError::rejected(
            "checkpoint_store_invalid",
            "checkpoint store is not a directory",
        ));
    }
    Ok(())
}

fn regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(archive_io("set restored mode"))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, destination)
}

#[cfg(not(unix))]
fn create_symlink(_target: &Path, _destination: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("checkpoint symlinks require Unix"))
}

fn invalid(detail: &'static str) -> RuntimeError {
    RuntimeError::rejected("checkpoint_invalid", detail)
}

fn archive_io(operation: &'static str) -> impl FnOnce(std::io::Error) -> RuntimeError + Copy {
    move |error| RuntimeError::unavailable("checkpoint_io_failed", format!("{operation}: {error}"))
}

fn stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

struct Cleanup(Option<PathBuf>);

impl Cleanup {
    fn new(path: PathBuf) -> Self {
        Self(Some(path))
    }

    fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = if path.is_dir() {
                std::fs::remove_dir_all(path)
            } else {
                std::fs::remove_file(path)
            };
        }
    }
}

#[cfg(test)]
#[path = "checkpoint_archive_tests.rs"]
mod tests;
