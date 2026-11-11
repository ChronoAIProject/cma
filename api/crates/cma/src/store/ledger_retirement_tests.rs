use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};

struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("cma-retirement-output-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn output_is_private_and_existing_file_or_symlink_is_never_overwritten() {
    let fixture = Fixture::new();
    let destination = fixture.0.join("snapshot.json");
    publish(&destination, b"fixture observation").unwrap();
    assert_eq!(
        std::fs::metadata(&destination)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(publish(&destination, b"replacement"), Err(OUTPUT));
    let link = fixture.0.join("alias.json");
    symlink(&destination, &link).unwrap();
    assert_eq!(publish(&link, b"replacement"), Err(OUTPUT));
    let dangling = fixture.0.join("dangling.json");
    let absent = fixture.0.join("absent.json");
    symlink(&absent, &dangling).unwrap();
    assert_eq!(publish(&dangling, b"replacement"), Err(OUTPUT));
    assert!(!absent.exists());
    assert_eq!(std::fs::read(destination).unwrap(), b"fixture observation");
}

#[test]
fn source_evidence_bounds_bytes_and_separates_collection_domains() {
    let document = doc! { "fixture": "observation" };
    let mut first = Evidence {
        digest: Sha256::new(),
        bytes: 0,
    };
    let mut second = Evidence {
        digest: Sha256::new(),
        bytes: 0,
    };
    first.record("agents", &document).unwrap();
    second.record("worktrees", &document).unwrap();
    assert_ne!(first.digest.finalize(), second.digest.finalize());
    let mut full = Evidence {
        digest: Sha256::new(),
        bytes: MAX_SOURCE_BYTES,
    };
    assert_eq!(full.record("agents", &document), Err(INVALID));
}
