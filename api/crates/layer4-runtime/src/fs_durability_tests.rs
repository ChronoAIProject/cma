use super::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

// A conditional storage-recovery model: discard entries without parent sync
// and regular files without file sync. This does not simulate real power loss.
pub(crate) struct Recovery {
    root: PathBuf,
    entries: BTreeSet<PathBuf>,
    files: BTreeSet<PathBuf>,
}

impl Recovery {
    pub(crate) fn new(root: &Path) -> Self {
        Self {
            root: root.canonicalize().unwrap(),
            entries: BTreeSet::new(),
            files: BTreeSet::new(),
        }
    }

    pub(crate) fn directory(&mut self, path: &Path) -> io::Result<()> {
        sync_directory(path)?;
        let path = path.canonicalize()?;
        if path.starts_with(&self.root) {
            for entry in std::fs::read_dir(path)? {
                self.entries.insert(entry?.path());
            }
        }
        Ok(())
    }

    pub(crate) fn file(&mut self, path: &Path, file: &File) -> io::Result<()> {
        file.sync_all()?;
        self.files.insert(path.canonicalize()?);
        Ok(())
    }

    pub(crate) fn recover(&self) {
        let mut directories = vec![self.root.clone()];
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let kind = entry.file_type().unwrap();
                if !self.entries.contains(&path) || (kind.is_file() && !self.files.contains(&path))
                {
                    if kind.is_dir() {
                        std::fs::remove_dir_all(path).unwrap();
                    } else {
                        std::fs::remove_file(path).unwrap();
                    }
                } else if kind.is_dir() {
                    directories.push(path);
                }
            }
        }
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-directory-sync-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn directory_chain_repairs_partial_creation_at_every_ancestor() {
    for repair in [false, true] {
        for suffix in ["", "one", "one/two", "one/two/three"] {
            let fixture = Fixture::new();
            let leaf = fixture.0.join("one/two/three/four");
            let failed = fixture.0.join(suffix);
            let mut recovery = Recovery::new(&fixture.0);
            let result = create_dir_all_with(&leaf, &mut |path| {
                if path == failed {
                    Err(io::Error::other("injected directory sync failure"))
                } else {
                    recovery.directory(path)
                }
            });
            assert!(result.is_err());
            assert!(leaf.is_dir(), "mkdir completed before the failed sync");
            // The existing missing-only helper is the negative control: its
            // successful retry alone does not repair the unsynced entries.
            create_dir_all_with(&leaf, &mut |path| recovery.directory(path)).unwrap();
            if repair {
                let mut order = Vec::new();
                sync_directory_chain(&leaf, &mut |path| {
                    order.push(path.to_path_buf());
                    recovery.directory(path)
                })
                .unwrap();
                let expected: Vec<_> = leaf
                    .ancestors()
                    .take_while(|path| path.starts_with(&fixture.0))
                    .map(Path::to_path_buf)
                    .collect();
                assert!(order.starts_with(&expected));
                assert!(order.iter().any(|path| path == Path::new("/")));
            }
            recovery.recover();
            assert_eq!(leaf.is_dir(), repair);
        }
    }
}

#[test]
fn directory_chain_syncs_temporary_alias_and_target_ancestors() {
    let fixture = Fixture::new();
    let target = fixture.0.join("physical/nested/temporary");
    std::fs::create_dir_all(&target).unwrap();
    let alias_parent = fixture.0.join("aliases");
    std::fs::create_dir(&alias_parent).unwrap();
    let intermediate = fixture.0.join("alias-targets");
    std::fs::create_dir(&intermediate).unwrap();
    std::os::unix::fs::symlink(&target, intermediate.join("hop")).unwrap();
    let alias = alias_parent.join("temporary");
    std::os::unix::fs::symlink("../alias-targets/hop", &alias).unwrap();
    let leaf = alias.join("checkpoints/tokens");
    std::fs::create_dir_all(&leaf).unwrap();
    let mut recovery = Recovery::new(&fixture.0);
    sync_directory_chain(&leaf, &mut |path| recovery.directory(path)).unwrap();
    recovery.recover();
    assert!(leaf.is_dir());
    assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
    assert!(intermediate.join("hop").is_dir());
}

#[test]
fn directory_chain_rejects_a_symlink_or_regular_file_at_the_leaf() {
    let fixture = Fixture::new();
    let leaf = fixture.0.join("leaf");
    std::fs::write(&leaf, b"").unwrap();
    assert!(sync_directory_chain(&leaf, &mut sync_directory).is_err());
    std::fs::remove_file(&leaf).unwrap();
    std::os::unix::fs::symlink(&fixture.0, &leaf).unwrap();
    assert!(sync_directory_chain(&leaf, &mut sync_directory).is_err());
}
