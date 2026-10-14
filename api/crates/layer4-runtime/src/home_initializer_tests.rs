use super::*;
use std::path::PathBuf;

struct Fixture {
    base: PathBuf,
    root: PathBuf,
    seed: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-home-init-{}", uuid::Uuid::new_v4().simple()));
        let root = base.join("volume");
        let seed = base.join("image-home");
        fs::create_dir_all(&root).unwrap();
        for name in ["example-one", "example-two"] {
            let skill = seed.join(".skills").join(name);
            fs::create_dir_all(&skill).unwrap();
            fs::write(skill.join("SKILL.md"), format!("public skill: {name}\n")).unwrap();
        }
        fs::create_dir_all(seed.join(".cma/agents")).unwrap();
        fs::write(
            seed.join(".profile"),
            "export PATH=$HOME/.local/bin:$PATH\n",
        )
        .unwrap();
        Self { base, root, seed }
    }

    fn run(&self) -> io::Result<()> {
        initialize(&self.root, &self.seed)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

#[test]
fn empty_volume_copies_public_home_and_keeps_retained_work_untouched() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("lost+found")).unwrap();
    f.run().unwrap();
    assert!(same_tree(&f.seed, &f.root.join("home")).unwrap());
    for name in ["state", "transfer", COMPLETE] {
        ensure_empty(&f.root.join(name)).unwrap();
    }
    fs::write(
        f.root.join("home/.profile"),
        "owner modified shell config\n",
    )
    .unwrap();
    fs::write(f.root.join("home/work.txt"), "uncommitted work").unwrap();
    fs::write(f.root.join("state/binding.json"), "retained state").unwrap();
    fs::write(f.root.join("transfer/data"), "retained transfer").unwrap();
    // A restarted init must not depend on the seed or overwrite mounted data.
    fs::remove_dir_all(&f.seed).unwrap();
    f.run().unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("home/work.txt")).unwrap(),
        "uncommitted work"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("state/binding.json")).unwrap(),
        "retained state"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("home/.profile")).unwrap(),
        "owner modified shell config\n"
    );
}

#[test]
fn retries_recover_partial_copy_and_home_publication_before_completion() {
    let f = Fixture::new();
    private_directory(&f.root.join(STAGING)).unwrap();
    fs::write(f.root.join(STAGING).join("interrupted-copy"), "partial").unwrap();
    f.run().unwrap();
    assert!(same_tree(&f.seed, &f.root.join("home")).unwrap());
    fs::remove_dir(f.root.join(COMPLETE)).unwrap();
    fs::remove_dir(f.root.join("transfer")).unwrap();
    f.run().unwrap();
    ensure_empty(&f.root.join("transfer")).unwrap();
    assert!(f.root.join(COMPLETE).is_dir());
}

#[test]
fn missing_completion_never_reseeds_existing_work_or_state() {
    for name in ["home/work.txt", "state/binding.json", "transfer/data"] {
        let f = Fixture::new();
        f.run().unwrap();
        fs::remove_dir(f.root.join(COMPLETE)).unwrap();
        let path = f.root.join(name);
        fs::write(&path, "must survive refusal").unwrap();
        assert!(
            f.run().is_err(),
            "must reject unexpected contents in {name}"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "must survive refusal");
        assert!(!f.root.join(COMPLETE).exists());
    }
}

#[test]
fn missing_or_non_directory_image_source_cannot_initialize_a_volume() {
    let f = Fixture::new();
    fs::remove_dir_all(&f.seed).unwrap();
    assert!(f.run().is_err());
    fs::write(&f.seed, "not a directory").unwrap();
    assert!(f.run().is_err());
    assert!(!f.root.join(COMPLETE).exists());
    assert!(!f.root.join("home").exists());
}

#[test]
fn unknown_layout_and_malformed_completion_are_refused() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("another-application")).unwrap();
    assert!(f.run().is_err());
    fs::remove_dir(f.root.join("another-application")).unwrap();
    f.run().unwrap();
    fs::write(f.root.join(COMPLETE).join("unexpected"), "data").unwrap();
    assert!(f.run().is_err());
    assert!(initialize(Path::new("/"), &f.seed).is_err());
}

#[cfg(unix)]
#[test]
fn symlinked_layout_is_refused_but_retained_user_symlinks_are_preserved() {
    use std::os::unix::fs::symlink;
    for name in ["home", "state", "transfer", COMPLETE, STAGING, "lost+found"] {
        let f = Fixture::new();
        symlink(&f.seed, f.root.join(name)).unwrap();
        assert!(f.run().is_err(), "must reject layout symlink {name}");
        assert!(f.seed.join(".profile").exists());
    }
    let f = Fixture::new();
    symlink(&f.root, f.base.join("linked-root")).unwrap();
    assert!(initialize(&f.base.join("linked-root"), &f.seed).is_err());
    f.run().unwrap();
    symlink("work.txt", f.root.join("home/user-link")).unwrap();
    f.run().unwrap();
    assert_eq!(
        fs::read_link(f.root.join("home/user-link")).unwrap(),
        Path::new("work.txt")
    );
}

#[cfg(unix)]
#[test]
fn public_seed_symlinks_are_not_followed_or_published() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    let other = f.base.join("private");
    fs::write(&other, "do not copy").unwrap();
    symlink(&other, f.seed.join("linked-file")).unwrap();
    assert!(f.run().is_err());
    assert!(!f.root.join(COMPLETE).exists());
    assert!(!f.root.join("home").exists());
    fs::remove_file(f.seed.join("linked-file")).unwrap();
    f.run().unwrap();
    assert!(!f.root.join("home/linked-file").exists());
}
