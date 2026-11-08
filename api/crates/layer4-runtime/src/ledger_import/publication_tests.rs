use super::*;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const COMPLETE: &[u8] = b"{\"complete\":true}";
const REPLACEMENT: &[u8] = b"foreign descriptor must survive";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("ledger-publication-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn directory(&self) -> File {
        File::open(&self.0).unwrap()
    }
    fn temporary(&self) -> PathBuf {
        std::fs::read_dir(&self.0)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with(".publish-")
            })
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn contents(file: &mut File) -> Result<()> {
    file.write_all(COMPLETE).map_err(|_| storage())
}
fn identity(path: &Path) -> (u64, u64) {
    let meta = std::fs::symlink_metadata(path).unwrap();
    (meta.dev(), meta.ino())
}
fn original_error(error: crate::failure::RuntimeError) {
    let original = invalid();
    assert_eq!(error.code, original.code);
    assert_eq!(error.detail, original.detail);
    assert_eq!(error.retryable, original.retryable);
}

#[test]
fn descriptor_publication_neutralizes_ready_and_seal_at_each_failed_barrier() {
    for name in ["ready.json", "sealed.json"] {
        for failure in [
            Step::FileSync,
            Step::Rename,
            Step::FinalSync,
            Step::DirectorySync,
            Step::Verify,
        ] {
            let fixture = Fixture::new();
            let result = write(
                &fixture.directory(),
                name,
                contents,
                || Ok(()),
                |step| {
                    if step == failure {
                        Err(storage())
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(result.unwrap_err().code, "mutation_ledger_storage_hold");
            let entries = std::fs::read_dir(&fixture.0)
                .unwrap()
                .collect::<std::io::Result<Vec<_>>>()
                .unwrap();
            assert_eq!(entries.len(), 1);
            let artifact = entries[0].path();
            let bytes = std::fs::read(&artifact).unwrap();
            assert!(bytes.is_empty(), "{name}: {failure:?}");
            assert!(serde_json::from_slice::<serde_json::Value>(&bytes).is_err());
            assert_eq!(
                artifact == fixture.0.join(name),
                !matches!(failure, Step::FileSync | Step::Rename)
            );
        }
    }
}

#[test]
fn descriptor_publication_preserves_success_and_existing_descriptor_on_retry() {
    for name in ["ready.json", "sealed.json"] {
        let fixture = Fixture::new();
        let directory = fixture.directory();
        write(&directory, name, contents, || Ok(()), |_| Ok(())).unwrap();
        let path = fixture.0.join(name);
        let before = identity(&path);
        assert_eq!(std::fs::read(&path).unwrap(), COMPLETE);
        assert_eq!(std::fs::metadata(&path).unwrap().mode() & 0o777, 0o400);
        assert!(write(&directory, name, contents, || Ok(()), |_| Ok(())).is_err());
        assert_eq!(identity(&path), before);
        assert_eq!(std::fs::read(path).unwrap(), COMPLETE);
        assert!(std::fs::read(fixture.temporary()).unwrap().is_empty());
    }
}

#[test]
fn withdrawal_preserves_replaced_ready_and_sealed_descriptors() {
    for name in ["ready.json", "sealed.json"] {
        for replace_at_cleanup in [false, true] {
            let fixture = Fixture::new();
            let path = fixture.0.join(name);
            let owned = fixture.0.join("detached-owned-file");
            let before = std::cell::Cell::new(None);
            let replace = || {
                std::fs::rename(&path, &owned).unwrap();
                std::fs::write(&path, REPLACEMENT).unwrap();
                before.set(Some(identity(&path)));
            };
            let error = write(
                &fixture.directory(),
                name,
                contents,
                || {
                    if !replace_at_cleanup {
                        replace();
                    }
                    Err(invalid())
                },
                |step| {
                    if replace_at_cleanup && step == Step::Neutralize {
                        replace();
                    }
                    Ok(())
                },
            )
            .unwrap_err();
            original_error(error);
            assert_eq!(Some(identity(&path)), before.get());
            assert_eq!(std::fs::read(path).unwrap(), REPLACEMENT);
            assert!(std::fs::read(owned).unwrap().is_empty());
        }
    }
}

#[test]
fn withdrawal_preserves_replaced_temporary_descriptors() {
    for name in ["ready.json", "sealed.json"] {
        let fixture = Fixture::new();
        let owned = fixture.0.join("detached-owned-file");
        let mut replacement = None;
        let error = write(
            &fixture.directory(),
            name,
            contents,
            || Ok(()),
            |step| {
                if step == Step::Rename {
                    let path = fixture.temporary();
                    std::fs::rename(&path, &owned).unwrap();
                    std::fs::write(&path, REPLACEMENT).unwrap();
                    replacement = Some((path.clone(), identity(&path)));
                    return Err(invalid());
                }
                Ok(())
            },
        )
        .unwrap_err();
        original_error(error);
        let (path, before) = replacement.unwrap();
        assert_eq!(identity(&path), before);
        assert_eq!(std::fs::read(path).unwrap(), REPLACEMENT);
        assert!(std::fs::read(owned).unwrap().is_empty());
        assert!(!fixture.0.join(name).exists());
    }
}

#[test]
fn withdrawal_uses_owned_fd_when_replacement_cannot_be_unlinked_as_a_file() {
    // Safe-fallback control: unlinkat(name, 0) would fail for the replacement
    // directory. Neutralization remains tied to the inode moved aside.
    for name in ["ready.json", "sealed.json"] {
        let fixture = Fixture::new();
        let path = fixture.0.join(name);
        let owned = fixture.0.join("detached-owned-file");
        let before = std::cell::Cell::new(None);
        let error = write(
            &fixture.directory(),
            name,
            contents,
            || {
                std::fs::rename(&path, &owned).unwrap();
                std::fs::create_dir(&path).unwrap();
                std::fs::write(path.join("preserve"), REPLACEMENT).unwrap();
                before.set(Some(identity(&path)));
                Err(invalid())
            },
            |_| Ok(()),
        )
        .unwrap_err();
        original_error(error);
        assert_eq!(Some(identity(&path)), before.get());
        assert_eq!(std::fs::read(path.join("preserve")).unwrap(), REPLACEMENT);
        assert!(std::fs::read(owned).unwrap().is_empty());
    }
}

struct LogBuffer(Arc<Mutex<Vec<u8>>>);
impl Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn failed_neutralization_reports_cleanup_failure_and_preserves_original_error() {
    for name in ["ready.json", "sealed.json"] {
        for (failure, phase) in [
            (Step::Neutralize, "neutralize_descriptor"),
            (Step::NeutralizeSync, "persist_neutralization"),
        ] {
            let fixture = Fixture::new();
            let captured = Arc::new(Mutex::new(Vec::new()));
            let buffer = Arc::clone(&captured);
            let subscriber = tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_writer(move || LogBuffer(Arc::clone(&buffer)))
                .finish();
            let error = tracing::subscriber::with_default(subscriber, || {
                write(
                    &fixture.directory(),
                    name,
                    contents,
                    || Err(invalid()),
                    |step| {
                        if step == failure {
                            Err(storage())
                        } else {
                            Ok(())
                        }
                    },
                )
                .unwrap_err()
            });
            original_error(error);
            let log = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
            assert!(log.contains(phase) && log.contains("ledger publication cleanup failed"));
            assert!(!log.contains(name) && !log.contains(fixture.0.to_str().unwrap()));
            let bytes = std::fs::read(fixture.0.join(name)).unwrap();
            assert_eq!(
                bytes.as_slice(),
                if failure == Step::Neutralize {
                    COMPLETE
                } else {
                    b""
                }
            );
        }
    }
}
