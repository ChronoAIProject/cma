use super::*;
use crate::fs_durability::tests::Recovery;
use std::cell::RefCell;

struct Fixture {
    root: PathBuf,
    config: RuntimeConfig,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("cma-token-retire-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let mut config = RuntimeConfig::for_test(root.join("home"));
        config.checkpoint_root = root.join("one/two/checkpoints");
        Self { root, config }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn directory_create_retry_cannot_skip_a_failed_ancestor_sync() {
    for suffix in ["", "one", "one/two", "one/two/checkpoints"] {
        let fixture = Fixture::new();
        let failure = fixture.root.join(suffix);
        let token = "a".repeat(64);
        let mut failures = 0;
        let mut sync = |path: &Path| {
            if path == failure {
                failures += 1;
                Err(io::Error::other("injected directory sync failure"))
            } else {
                crate::fs_durability::sync_directory(path)
            }
        };
        for attempt in 1..=2 {
            let result = retire_with(&fixture.config, &token, &mut File::sync_all, &mut sync);
            assert!(
                result.is_err(),
                "attempt {attempt} acknowledged an unsynced ancestor"
            );
            assert_eq!(
                result.unwrap_err().code,
                "checkpoint_writer_state_unavailable"
            );
            assert!(fixture.config.checkpoint_root.is_dir());
        }
        assert_eq!(failures, 2);
        retire(&fixture.config, &token).unwrap();
        assert!(was_resumed(&fixture.config, &token).unwrap());
        assert!(!was_resumed(&fixture.config, &"b".repeat(64)).unwrap());
    }
}

#[test]
fn retired_marker_survives_modeled_recovery_after_each_directory_retry() {
    for suffix in [
        None,
        Some(""),
        Some("one"),
        Some("one/two"),
        Some("one/two/checkpoints"),
        Some("one/two/checkpoints/checkpoint-writer-state"),
    ] {
        let fixture = Fixture::new();
        let token = "c".repeat(64);
        let marker = fixture
            .config
            .checkpoint_root
            .join("checkpoint-writer-state")
            .join(&token);
        let recovery = RefCell::new(Recovery::new(&fixture.root));
        let mut failed = suffix.map(|suffix| fixture.root.join(suffix));
        let first = retire_with(
            &fixture.config,
            &token,
            &mut |file| recovery.borrow_mut().file(&marker, file),
            &mut |path| {
                if failed.as_deref() == Some(path) {
                    failed.take();
                    Err(io::Error::other("injected directory sync failure"))
                } else {
                    recovery.borrow_mut().directory(path)
                }
            },
        );
        assert_eq!(first.is_ok(), suffix.is_none());
        if suffix.is_some() {
            assert!(marker.is_file(), "retry must exercise the existing marker");
            retire_with(
                &fixture.config,
                &token,
                &mut |file| recovery.borrow_mut().file(&marker, file),
                &mut |path| recovery.borrow_mut().directory(path),
            )
            .unwrap();
        }
        recovery.borrow().recover();
        assert!(was_resumed(&fixture.config, &token).unwrap());
        assert_eq!(std::fs::metadata(marker).unwrap().len(), 0);
        assert!(!was_resumed(&fixture.config, &"d".repeat(64)).unwrap());
    }
}

#[test]
fn failed_retirement_has_no_success_guarantee_in_the_recovery_model() {
    let fixture = Fixture::new();
    let token = "e".repeat(64);
    let marker = fixture
        .config
        .checkpoint_root
        .join("checkpoint-writer-state")
        .join(&token);
    let recovery = RefCell::new(Recovery::new(&fixture.root));
    let failure = retire_with(
        &fixture.config,
        &token,
        &mut |file| recovery.borrow_mut().file(&marker, file),
        &mut |path| {
            if path == fixture.root {
                Err(io::Error::other("injected directory sync failure"))
            } else {
                recovery.borrow_mut().directory(path)
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, "checkpoint_writer_state_unavailable");
    assert!(
        was_resumed(&fixture.config, &token).unwrap(),
        "visible is not durable"
    );
    recovery.borrow().recover();
    assert!(!was_resumed(&fixture.config, &token).unwrap());
}

#[test]
fn marker_sync_failures_remain_retryable_for_new_and_existing_markers() {
    let fixture = Fixture::new();
    let token = "f".repeat(64);
    for _ in 0..2 {
        let failure = retire_with(
            &fixture.config,
            &token,
            &mut |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "synthetic private path detail",
                ))
            },
            &mut crate::fs_durability::sync_directory,
        )
        .unwrap_err();
        assert_eq!(failure.code, "checkpoint_writer_state_unavailable");
        assert!(failure.retryable);
        assert_eq!(
            failure.detail,
            "checkpoint writer state I/O failed: PermissionDenied"
        );
    }
    retire(&fixture.config, &token).unwrap();
    assert!(was_resumed(&fixture.config, &token).unwrap());
}

#[test]
fn token_directory_sync_failure_cannot_acknowledge_an_existing_marker() {
    let fixture = Fixture::new();
    let token = "a".repeat(64);
    retire(&fixture.config, &token).unwrap();
    let directory = fixture
        .config
        .checkpoint_root
        .join("checkpoint-writer-state");
    for _ in 0..2 {
        let failure = retire_with(&fixture.config, &token, &mut File::sync_all, &mut |path| {
            if path == directory {
                Err(io::Error::other("injected token directory sync failure"))
            } else {
                crate::fs_durability::sync_directory(path)
            }
        })
        .unwrap_err();
        assert_eq!(failure.code, "checkpoint_writer_state_unavailable");
    }
    retire(&fixture.config, &token).unwrap();
    assert!(was_resumed(&fixture.config, &token).unwrap());
}

#[test]
fn invalid_marker_and_directory_shapes_remain_rejected() {
    for location in ["root", "directory", "marker"] {
        for shape in ["file", "symlink", "directory"] {
            if shape == "directory" && location != "marker" {
                continue;
            }
            let fixture = Fixture::new();
            let token = "b".repeat(64);
            let directory = fixture
                .config
                .checkpoint_root
                .join("checkpoint-writer-state");
            let path = match location {
                "root" => fixture.config.checkpoint_root.clone(),
                "directory" => directory,
                _ => directory.join(&token),
            };
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            match shape {
                "file" => std::fs::write(&path, b"invalid record").unwrap(),
                "symlink" => std::os::unix::fs::symlink(&fixture.root, &path).unwrap(),
                _ => std::fs::create_dir(&path).unwrap(),
            }
            let failure = retire(&fixture.config, &token).unwrap_err();
            assert_eq!(failure.code, "checkpoint_writer_state_invalid");
            assert!(!failure.retryable);
            assert_eq!(
                was_resumed(&fixture.config, &token).unwrap_err().code,
                "checkpoint_writer_state_invalid"
            );
        }
    }
}

#[test]
fn configured_and_platform_temporary_paths_keep_their_layout() {
    let fixture = Fixture::new();
    let assigned = fixture.root.join("assigned/nested");
    std::fs::create_dir_all(&assigned).unwrap();
    let alias = fixture.root.join("temporary-alias");
    std::os::unix::fs::symlink(&assigned, &alias).unwrap();
    for temporary in [assigned, alias] {
        let config = RuntimeConfig::from_values(std::collections::BTreeMap::from([
            ("CHRONO_SANDBOX_ID".into(), "sandbox-test".into()),
            (
                "CHRONO_RUNTIME_HOME".into(),
                fixture.root.join("home").to_string_lossy().into_owned(),
            ),
            ("TMPDIR".into(), temporary.to_string_lossy().into_owned()),
        ]))
        .unwrap();
        assert_eq!(config.checkpoint_root, temporary.join("cma-checkpoints"));
        retire(&config, &"c".repeat(64)).unwrap();
        assert!(was_resumed(&config, &"c".repeat(64)).unwrap());
    }
    let mut config = RuntimeConfig::from_values(std::collections::BTreeMap::from([(
        "CHRONO_SANDBOX_ID".into(),
        "sandbox-test".into(),
    )]))
    .unwrap();
    assert_eq!(
        config.checkpoint_root,
        std::env::temp_dir().join("cma-checkpoints")
    );
    // Keep the platform's original alias-containing prefix, in a private fixture.
    config.checkpoint_root = std::env::temp_dir()
        .join(fixture.root.file_name().unwrap())
        .join("platform-checkpoints");
    retire(&config, &"d".repeat(64)).unwrap();
    assert!(was_resumed(&config, &"d".repeat(64)).unwrap());
}
