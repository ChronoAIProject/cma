use super::*;
use std::cell::{Cell, RefCell};

fn sync_error() -> std::io::Error {
    std::io::Error::from_raw_os_error(libc::EIO)
}

// Model independent file-data and directory-entry persistence. Real files
// provide visible bytes; recovery exposes only successfully synchronized data.
#[derive(Default)]
struct Persistence {
    data: RefCell<Option<Vec<u8>>>,
    entry: Cell<bool>,
}
impl Persistence {
    fn sync_file(&self, file: &File, path: &Path) -> std::io::Result<()> {
        file.sync_all()?;
        assert_eq!(file.metadata()?.ino(), std::fs::metadata(path)?.ino());
        *self.data.borrow_mut() = Some(std::fs::read(path)?);
        Ok(())
    }
    fn sync_parent(&self, parent: &Directory) -> Result<()> {
        parent.sync()?;
        self.entry.set(true);
        Ok(())
    }
    fn recover(&self) -> Option<Vec<u8>> {
        self.entry
            .get()
            .then(|| self.data.borrow().clone())
            .flatten()
    }
}

#[test]
fn failed_file_sync_retry_preserves_complete_bytes_after_modeled_recovery() {
    let temp = Temp::new();
    let path = temp.0.join("receipt");
    let bytes = b"complete evidence";
    assert!(publish_private_file_with_sync(
        &path,
        bytes,
        |_| Err(sync_error()),
        |_| panic!("failed file sync must stop before directory sync"),
    )
    .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    let persistence = Persistence::default();
    assert_eq!(persistence.recover(), None);
    publish_private_file_with_sync(
        &path,
        bytes,
        |file| persistence.sync_file(file, &path),
        |parent| persistence.sync_parent(parent),
    )
    .unwrap();
    assert_eq!(persistence.recover().as_deref(), Some(bytes.as_slice()));
}

#[test]
fn identical_replay_refuses_failed_file_sync_without_syncing_parent() {
    let temp = Temp::new();
    let path = temp.0.join("receipt");
    publish_private_file(&path, b"evidence").unwrap();
    let parent_synced = Cell::new(false);
    let result = publish_private_file_with_sync(
        &path,
        b"evidence",
        |_| Err(sync_error()),
        |_| {
            parent_synced.set(true);
            Ok(())
        },
    );
    assert!(result.is_err());
    assert!(!parent_synced.get());
    assert_eq!(std::fs::read(&path).unwrap(), b"evidence");
}

#[test]
fn failed_directory_sync_requires_both_barriers_on_retry() {
    for existing in [false, true] {
        let temp = Temp::new();
        let path = temp.0.join("receipt");
        if existing {
            publish_private_file(&path, b"evidence").unwrap();
        }
        let events = RefCell::new(Vec::new());
        let result = publish_private_file_with_sync(
            &path,
            b"evidence",
            |file| {
                events.borrow_mut().push("file");
                file.sync_all()
            },
            |_| {
                events.borrow_mut().push("directory failure");
                Err(INVALID)
            },
        );
        assert!(result.is_err());
        publish_private_file_with_sync(
            &path,
            b"evidence",
            |file| {
                events.borrow_mut().push("file");
                file.sync_all()
            },
            |parent| {
                events.borrow_mut().push("directory success");
                parent.sync()
            },
        )
        .unwrap();
        assert_eq!(
            *events.borrow(),
            ["file", "directory failure", "file", "directory success"]
        );
    }
}

#[test]
fn concurrent_replay_syncs_before_returning_while_creator_is_unsynced() {
    use std::{sync::mpsc::sync_channel, time::Duration};
    let temp = Temp::new();
    let path = temp.0.join("receipt");
    let bytes = b"complete evidence";
    let (written_tx, written_rx) = sync_channel(0);
    let (resume_tx, resume_rx) = sync_channel(0);
    std::thread::scope(|scope| {
        let creator_path = &path;
        let creator = scope.spawn(move || {
            publish_private_file_with_sync(
                creator_path,
                bytes,
                |_| {
                    written_tx.send(()).unwrap();
                    resume_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                    Err(sync_error())
                },
                |_| panic!("creator never synchronized the file"),
            )
        });
        written_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let persistence = Persistence::default();
        let result = publish_private_file_with_sync(
            &path,
            bytes,
            |file| persistence.sync_file(file, &path),
            |parent| persistence.sync_parent(parent),
        );
        let recovered = persistence.recover();
        resume_tx.send(()).unwrap();
        assert!(creator.join().unwrap().is_err());
        result.unwrap();
        assert_eq!(recovered.as_deref(), Some(bytes.as_slice()));
    });
}

#[test]
fn partial_and_different_replays_never_overwrite_or_sync_existing_evidence() {
    for existing in [b"part".as_slice(), b"different evidence".as_slice()] {
        let temp = Temp::new();
        let path = temp.0.join("receipt");
        publish_private_file(&path, existing).unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        assert!(publish_private_file_with_sync(
            &path,
            b"partial evidence",
            |_| panic!("different bytes must refuse before file sync"),
            |_| panic!("different bytes must refuse before directory sync"),
        )
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), existing);
        assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    }
}
