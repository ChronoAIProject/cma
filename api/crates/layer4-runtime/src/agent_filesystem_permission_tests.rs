use super::*;
use std::{os::unix::process::CommandExt as _, process::Command};

#[test]
fn file_edits_preserve_modes_with_umask_022() {
    with_child_mask("file_edits_preserve_modes_with_umask_022", 0o022);
}

#[test]
fn file_edits_preserve_modes_with_umask_077() {
    with_child_mask("file_edits_preserve_modes_with_umask_077", 0o077);
}

fn with_child_mask(test: &str, mask: libc::mode_t) {
    const CHILD: &str = "CMA_FILE_PERMISSION_TEST_CHILD";
    if let Some(selected) = std::env::var_os(CHILD) {
        assert_eq!(selected, test);
        check_modes(mask);
        return;
    }
    let test_name = format!("{}::{test}", module_path!().split_once("::").unwrap().1);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", &test_name, "--nocapture", "--test-threads=1"])
        .env(CHILD, test);
    // SAFETY: umask is async-signal-safe. Only the fresh child changes its mask;
    // the parallel parent harness and other tests retain their own mask.
    unsafe {
        command.pre_exec(move || {
            libc::umask(mask);
            Ok(())
        });
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "permission child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
        "child must execute exactly one permission test"
    );
}

fn check_modes(mask: libc::mode_t) {
    let fixture = Fixture::new();
    fixture.write("new.txt", b"new file", None).unwrap();
    assert_eq!(mode(&fixture, "new.txt"), 0o600 & !u32::from(mask));

    // Read-only, executable, shared and private files retain ordinary bits;
    // set-user-ID is excluded from the replacement as before.
    for original in [0o444, 0o755, 0o770, 0o640, 0o600, 0o4755] {
        let path = format!("mode-{original:o}");
        let first = fixture.write(&path, b"before edit", None).unwrap();
        std::fs::set_permissions(
            fixture.0.home.join(&path),
            std::fs::Permissions::from_mode(original),
        )
        .unwrap();
        assert_eq!(mode(&fixture, &path), original);
        fixture
            .write(&path, b"after edit", Some(first.sha256))
            .unwrap();
        assert_eq!(mode(&fixture, &path), original & 0o777);
        assert_eq!(
            fixture.read(&path, 1024).unwrap().content().unwrap(),
            b"after edit"
        );
    }
}

fn mode(fixture: &Fixture, path: &str) -> u32 {
    std::fs::metadata(fixture.0.home.join(path))
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}
