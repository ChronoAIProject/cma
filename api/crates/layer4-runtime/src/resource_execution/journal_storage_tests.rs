use super::*;
use std::io::Write;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let fixture = Self(std::env::temp_dir().join(format!(
            "cma-resource-journal-{}",
            uuid::Uuid::new_v4().simple()
        )));
        std::fs::create_dir_all(fixture.root()).unwrap();
        std::fs::write(fixture.0.join("receiver.activated"), b"marker").unwrap();
        std::fs::write(fixture.root().join("writer.lock"), b"").unwrap();
        std::fs::write(fixture.root().join("state.json"), b"original").unwrap();
        fixture
    }

    fn root(&self) -> PathBuf {
        self.0.join("receiver")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn replacement_during_save_refuses_publication_and_keeps_cleanup_in_retained_directory() {
    let fixture = Fixture::new();
    let storage = Storage::open(&fixture.root()).unwrap();
    let mut stage = storage.create(c"pending.tmp").unwrap();
    stage.write_all(b"pending").unwrap();
    stage.sync_all().unwrap();
    let retained = fixture.0.join("retained");
    std::fs::rename(fixture.root(), &retained).unwrap();
    std::fs::create_dir(fixture.root()).unwrap();
    std::fs::write(fixture.root().join("state.json"), b"replacement").unwrap();
    std::fs::write(fixture.root().join("pending.tmp"), b"unrelated").unwrap();

    assert!(storage.publish(c"pending.tmp").is_err());
    storage.remove_temporary(c"pending.tmp");
    assert!(!retained.join("pending.tmp").exists());
    assert_eq!(
        std::fs::read(retained.join("state.json")).unwrap(),
        b"original"
    );
    assert_eq!(
        std::fs::read(fixture.root().join("state.json")).unwrap(),
        b"replacement"
    );
    assert_eq!(
        std::fs::read(fixture.root().join("pending.tmp")).unwrap(),
        b"unrelated"
    );
}

#[test]
fn journal_regular_files_refuse_symlinks_and_additional_hard_links() {
    for name in [
        "receiver.activated",
        "receiver/writer.lock",
        "receiver/state.json",
    ] {
        for symlink in [false, true] {
            let fixture = Fixture::new();
            let path = fixture.0.join(name);
            let alias = fixture.0.join("alias");
            if symlink {
                std::fs::rename(&path, &alias).unwrap();
                std::os::unix::fs::symlink(&alias, &path).unwrap();
            } else {
                std::fs::hard_link(&path, &alias).unwrap();
            }
            let result = Storage::open(&fixture.root()).and_then(|storage| storage.state(4096));
            assert!(result.is_err(), "accepted {name}, symlink={symlink}");
        }
    }
}
