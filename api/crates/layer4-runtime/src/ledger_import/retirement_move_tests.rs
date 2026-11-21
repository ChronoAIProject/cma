use super::*;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

struct Fixture {
    root: PathBuf,
    active: PathBuf,
    retained: PathBuf,
    name: String,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("retirement-transfer-{}", uuid::Uuid::new_v4()));
        let active = root.join("active");
        let retained = root.join("retained");
        for path in [&root, &active, &retained] {
            std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
        }
        Self {
            root,
            active,
            retained,
            name: format!("{}.json", "a".repeat(64)),
        }
    }
    fn write(&self, directory: &Path, name: &str, value: &[u8]) {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(directory.join(name))
            .unwrap();
        file.write_all(value).unwrap();
        file.sync_all().unwrap();
    }
    fn pair(&self) -> Pair {
        Pair::open(&self.active, &self.retained).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

#[test]
fn interruption_at_each_move_boundary_resumes_and_restores_exact_bytes() {
    for stop in [
        Boundary::SourceFileSync,
        Boundary::BeforeMove,
        Boundary::AfterMove,
        Boundary::DestinationFileSync,
        Boundary::DestinationSync,
        Boundary::SourceSync,
        Boundary::Verified,
    ] {
        for restore in [false, true] {
            let fixture = Fixture::new();
            let bytes = b"exact archived fixture bytes";
            fixture.write(&fixture.active, &fixture.name, bytes);
            if restore {
                fixture
                    .pair()
                    .transfer(&fixture.name, bytes, false, |_| Ok(()))
                    .unwrap();
            }
            assert!(fixture
                .pair()
                .transfer(&fixture.name, bytes, restore, |step| {
                    if step == stop {
                        Err(storage())
                    } else {
                        Ok(())
                    }
                })
                .is_err());
            // Reopen descriptors as a restarted maintenance process would.
            for _ in 0..2 {
                fixture
                    .pair()
                    .transfer(&fixture.name, bytes, restore, |_| Ok(()))
                    .unwrap();
            }
            let (present, absent) = if restore {
                (&fixture.active, &fixture.retained)
            } else {
                (&fixture.retained, &fixture.active)
            };
            assert_eq!(std::fs::read(present.join(&fixture.name)).unwrap(), bytes);
            assert!(!absent.join(&fixture.name).exists());
        }
    }
}

#[test]
fn changed_missing_duplicate_and_aliased_records_are_preserved_or_refused() {
    for case in 0..7 {
        let fixture = Fixture::new();
        let bytes = b"expected bytes";
        match case {
            0 => fixture.write(&fixture.active, &fixture.name, b"changed bytes"),
            1 => {}
            2 => {
                fixture.write(&fixture.active, &fixture.name, bytes);
                fixture.write(&fixture.retained, &fixture.name, bytes);
            }
            3 => {
                fixture.write(&fixture.active, &fixture.name, bytes);
                fixture.write(&fixture.retained, &fixture.name, b"unrelated bytes");
            }
            4 => {
                fixture.write(&fixture.active, "alias", bytes);
                std::os::unix::fs::symlink(
                    fixture.active.join("alias"),
                    fixture.active.join(&fixture.name),
                )
                .unwrap();
            }
            5 => {
                fixture.write(&fixture.active, &fixture.name, bytes);
                std::fs::hard_link(
                    fixture.active.join(&fixture.name),
                    fixture.active.join("alias"),
                )
                .unwrap();
            }
            _ => {
                fixture.write(&fixture.active, &fixture.name, bytes);
                std::fs::create_dir(fixture.retained.join(&fixture.name)).unwrap();
            }
        }
        let before = std::fs::read(fixture.active.join(&fixture.name)).ok();
        let retained = std::fs::read(fixture.retained.join(&fixture.name)).ok();
        assert!(
            fixture
                .pair()
                .transfer(&fixture.name, bytes, false, |_| Ok(()))
                .is_err(),
            "case {case}"
        );
        assert_eq!(
            std::fs::read(fixture.active.join(&fixture.name)).ok(),
            before,
            "case {case}"
        );
        assert_eq!(
            std::fs::read(fixture.retained.join(&fixture.name)).ok(),
            retained,
            "case {case}"
        );
    }
}

#[test]
fn replacement_of_a_source_or_directory_never_qualifies_a_move() {
    for replace_directory in [false, true] {
        let fixture = Fixture::new();
        let bytes = b"expected bytes";
        fixture.write(&fixture.active, &fixture.name, bytes);
        let pair = fixture.pair();
        assert!(pair
            .transfer(&fixture.name, bytes, false, |step| {
                if step == Boundary::BeforeMove {
                    if replace_directory {
                        std::fs::rename(&fixture.active, fixture.root.join("original")).unwrap();
                        std::fs::DirBuilder::new()
                            .mode(0o700)
                            .create(&fixture.active)
                            .unwrap();
                        fixture.write(&fixture.active, &fixture.name, bytes);
                    } else {
                        std::fs::rename(
                            fixture.active.join(&fixture.name),
                            fixture.root.join("original"),
                        )
                        .unwrap();
                        fixture.write(&fixture.active, &fixture.name, bytes);
                    }
                }
                Ok(())
            })
            .is_err());
        assert_eq!(
            std::fs::read(fixture.active.join(&fixture.name)).unwrap(),
            bytes
        );
        assert!(!fixture.retained.join(&fixture.name).exists());
    }
}

#[test]
fn restore_refuses_new_active_effects_without_overwriting_either_copy() {
    let fixture = Fixture::new();
    fixture.write(&fixture.retained, &fixture.name, b"archived");
    fixture.write(&fixture.active, &fixture.name, b"new active effect");
    assert!(fixture
        .pair()
        .transfer(&fixture.name, b"archived", true, |_| Ok(()))
        .is_err());
    assert_eq!(
        std::fs::read(fixture.retained.join(&fixture.name)).unwrap(),
        b"archived"
    );
    assert_eq!(
        std::fs::read(fixture.active.join(&fixture.name)).unwrap(),
        b"new active effect"
    );
}

#[test]
fn destination_created_after_inspection_is_never_overwritten() {
    let fixture = Fixture::new();
    fixture.write(&fixture.active, &fixture.name, b"archived");
    assert!(fixture
        .pair()
        .transfer(&fixture.name, b"archived", false, |step| {
            if step == Boundary::BeforeMove {
                fixture.write(&fixture.retained, &fixture.name, b"retain this conflict");
            }
            Ok(())
        })
        .is_err());
    assert_eq!(
        std::fs::read(fixture.active.join(&fixture.name)).unwrap(),
        b"archived"
    );
    assert_eq!(
        std::fs::read(fixture.retained.join(&fixture.name)).unwrap(),
        b"retain this conflict"
    );
}

#[test]
fn overlapping_trees_and_changed_directory_permissions_refuse() {
    let fixture = Fixture::new();
    assert!(Pair::open(&fixture.active, &fixture.active).is_err());
    assert!(Pair::open(&fixture.active, &fixture.root).is_err());
    let nested = fixture.active.join("nested");
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&nested)
        .unwrap();
    assert!(Pair::open(&fixture.active, &nested).is_err());
    fixture.write(&fixture.active, &fixture.name, b"retained");
    let pair = fixture.pair();
    std::fs::set_permissions(&fixture.retained, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(pair
        .transfer(&fixture.name, b"retained", false, |_| Ok(()))
        .is_err());
    assert_eq!(
        std::fs::read(fixture.active.join(&fixture.name)).unwrap(),
        b"retained"
    );
}
