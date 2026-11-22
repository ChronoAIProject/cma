use super::*;
use std::{collections::BTreeMap, os::unix::fs::FileExt as _, process::Command};

const MODE_TEST_UMASK: &str = "CMA_FILE_MODE_TEST_UMASK";
const MODE_CHILD: &str = "agent_filesystem::tests::file_mode::write_under_child_umask";

#[test]
fn mode_preservation_defaults_off_and_checks_the_opt_in() {
    const FLAG: &str = "CHRONO_RUNTIME_PRESERVE_FILE_MODE_ENABLED";
    let base = BTreeMap::from([("CHRONO_SANDBOX_ID".into(), "file-mode-fixture".into())]);
    assert!(
        !RuntimeConfig::from_values(base.clone())
            .unwrap()
            .preserve_file_mode
    );
    for (raw, enabled) in [("0", false), ("false", false), ("1", true), ("true", true)] {
        let mut values = base.clone();
        values.insert(FLAG.into(), raw.into());
        assert_eq!(
            RuntimeConfig::from_values(values)
                .unwrap()
                .preserve_file_mode,
            enabled
        );
    }
    let mut invalid = base;
    invalid.insert(FLAG.into(), "yes".into());
    assert_eq!(
        RuntimeConfig::from_values(invalid).unwrap_err(),
        format!("{FLAG} must be 0, 1, false or true")
    );
}

#[test]
fn permission_matrix_runs_in_separate_umask_processes() {
    let executable = std::env::current_exe().unwrap();
    for mask in ["022", "077"] {
        let output = Command::new("/bin/sh")
            .args(["-c", "umask \"$1\"; shift; exec \"$@\"", "file-mode", mask])
            .arg(&executable)
            .args(["--exact", MODE_CHILD, "--nocapture"])
            .env(MODE_TEST_UMASK, mask)
            .output()
            .unwrap();
        assert!(
            output.status.success()
                && String::from_utf8_lossy(&output.stdout).contains("1 passed; 0 failed"),
            "umask {mask}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn write_under_child_umask() {
    let Ok(mask) = std::env::var(MODE_TEST_UMASK) else {
        return;
    };
    let mask = u32::from_str_radix(&mask, 8).unwrap();
    for (enabled, mode) in [(true, 0o755), (true, 0o640), (true, 0o7755), (false, 0o755)] {
        let mut fixture = Fixture::new();
        fixture.0.preserve_file_mode = enabled;
        let original = fixture.write("edit", b"before", None).unwrap();
        assert_eq!(mode_of(&fixture, "edit"), 0o600);
        set_test_mode(&fixture, "edit", mode);
        fixture.write("edit", b"before", None).unwrap();
        assert_eq!(mode_of(&fixture, "edit"), mode);
        assert_eq!(
            fixture
                .write("edit", b"wrong revision", Some("0".repeat(64)))
                .unwrap_err()
                .code,
            "file_conflict"
        );
        let written = fixture
            .write("edit", b"after", Some(original.sha256))
            .unwrap();
        let ordinary = mode & 0o777;
        assert_eq!(
            mode_of(&fixture, "edit"),
            if enabled { ordinary } else { ordinary & !mask }
        );
        let published = fixture.read("edit", 20).unwrap().content().unwrap();
        assert_eq!(published, b"after");
        assert_eq!(written.size_bytes, published.len() as u64);
        assert_eq!(written.sha256, digest(&published));
        assert_no_staging(&fixture);
    }
}

#[test]
fn shared_writable_files_and_directories_refuse_mode_preservation() {
    for (file_mode, directory_mode) in [(0o664, 0o755), (0o606, 0o755), (0o755, 0o775)] {
        let mut fixture = Fixture::new();
        fixture.0.preserve_file_mode = true;
        let original = fixture.write("edit", b"before", None).unwrap();
        set_test_mode(&fixture, "edit", file_mode);
        set_test_mode(&fixture, "", directory_mode);
        assert_eq!(fixture.write("edit", b"before", None).unwrap(), original);
        let request = fixture.request("edit", b"after", Some(original.sha256));
        let error = write_with_mode_change(&fixture.0, request, |_, _| {
            panic!("a writable stage could become visible before publication")
        })
        .unwrap_err();
        assert_eq!(error.detail, "cannot preserve existing file access policy");
        assert_eq!(
            std::fs::read(fixture.0.home.join("edit")).unwrap(),
            b"before"
        );
        assert_eq!(mode_of(&fixture, "edit"), file_mode);
        assert_no_staging(&fixture);
    }
}

#[test]
fn changed_staged_bytes_refuse_publication_with_the_original_intact() {
    let mut fixture = Fixture::new();
    fixture.0.preserve_file_mode = true;
    let original = fixture.write("edit", b"before", None).unwrap();
    set_test_mode(&fixture, "edit", 0o755);
    let request = fixture.request("edit", b"after", Some(original.sha256));
    let error = write_with_mode_change(&fixture.0, request, |stage, mode| {
        existing_file::set_mode(stage, mode)?;
        stage.write_all_at(b"other", 0).unwrap();
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "file_conflict");
    assert_eq!(
        std::fs::read(fixture.0.home.join("edit")).unwrap(),
        b"before"
    );
    assert_eq!(mode_of(&fixture, "edit"), 0o755);
    assert_no_staging(&fixture);
}

#[test]
fn an_inherited_access_acl_refuses_the_edit_before_permissions_widen() {
    let mut fixture = Fixture::new();
    fixture.0.preserve_file_mode = true;
    let original = fixture.write("edit", b"before", None).unwrap();
    set_test_mode(&fixture, "edit", 0o755);

    let mut acl = 2u32.to_le_bytes().to_vec();
    // Linux posix_acl_xattr.h version 2 and a named user entry.
    for (tag, bits, id) in [
        (1u16, 7u16, u32::MAX),
        (2, 4, 42_420),
        (4, 5, u32::MAX),
        (16, 5, u32::MAX),
        (32, 5, u32::MAX),
    ] {
        acl.extend(tag.to_le_bytes());
        acl.extend(bits.to_le_bytes());
        acl.extend(id.to_le_bytes());
    }
    let directory = File::open(&fixture.0.home).unwrap();
    // SAFETY: the directory and ACL buffer are live for this test-only syscall.
    let result = unsafe {
        libc::fsetxattr(
            directory.as_raw_fd(),
            c"system.posix_acl_default".as_ptr(),
            acl.as_ptr().cast(),
            acl.len(),
            0,
        )
    };
    assert_eq!(result, 0, "{}", std::io::Error::last_os_error());
    let request = fixture.request("edit", b"after", Some(original.sha256));
    let error = write_with_mode_change(&fixture.0, request, |_, _| {
        panic!("mode widening reached a staged file with an inherited ACL")
    })
    .unwrap_err();
    assert_eq!(error.detail, "cannot preserve existing file access policy");
    assert_eq!(
        fixture.read("edit", 20).unwrap().content().unwrap(),
        b"before"
    );
    assert_eq!(mode_of(&fixture, "edit"), 0o755);
    assert_no_staging(&fixture);
}

#[test]
fn permission_change_failure_keeps_the_original_and_cleans_staging() {
    let mut fixture = Fixture::new();
    fixture.0.preserve_file_mode = true;
    let original = fixture.write("edit", b"before", None).unwrap();
    set_test_mode(&fixture, "edit", 0o755);
    let request = fixture.request("edit", b"after", Some(original.sha256));
    let error = write_with_mode_change(&fixture.0, request, |stage, _| {
        assert_eq!(stage.metadata().unwrap().permissions().mode() & 0o077, 0);
        Err(io_error(std::io::Error::from_raw_os_error(libc::EACCES)))
    })
    .unwrap_err();
    assert_eq!(error.code, "filesystem_permission");
    assert_eq!(
        fixture.read("edit", 20).unwrap().content().unwrap(),
        b"before"
    );
    assert_eq!(mode_of(&fixture, "edit"), 0o755);
    assert_no_staging(&fixture);
}

#[test]
fn rechecks_original_identity_and_permissions_before_publication() {
    for change in ["mode", "parent mode", "contents", "identity"] {
        let mut fixture = Fixture::new();
        fixture.0.preserve_file_mode = true;
        let original = fixture.write("edit", b"before", None).unwrap();
        set_test_mode(&fixture, "edit", 0o755);
        let request = fixture.request("edit", b"must not publish", Some(original.sha256));
        let error = write_with_mode_change(&fixture.0, request, |stage, mode| {
            existing_file::set_mode(stage, mode)?;
            let target = fixture.0.home.join("edit");
            match change {
                "mode" => set_test_mode(&fixture, "edit", 0o700),
                "parent mode" => set_test_mode(&fixture, "", 0o711),
                "contents" => std::fs::write(target, b"changed").unwrap(),
                "identity" => {
                    std::fs::rename(&target, fixture.0.home.join("old")).unwrap();
                    std::fs::write(target, b"changed").unwrap();
                }
                _ => unreachable!(),
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code, "file_conflict", "{change}");
        let expected: &[u8] = if matches!(change, "contents" | "identity") {
            b"changed"
        } else {
            b"before"
        };
        assert_eq!(
            fixture.read("edit", 25).unwrap().content().unwrap(),
            expected
        );
        assert_no_staging(&fixture);
    }
}

#[test]
fn replacing_the_parent_entry_never_publishes_into_the_moved_directory() {
    let mut fixture = Fixture::new();
    fixture.0.preserve_file_mode = true;
    let original = fixture.write("parent/edit", b"before", None).unwrap();
    set_test_mode(&fixture, "parent/edit", 0o755);
    let request = fixture.request("parent/edit", b"must not publish", Some(original.sha256));
    let error = write_with_mode_change(&fixture.0, request, |stage, mode| {
        existing_file::set_mode(stage, mode)?;
        let parent = fixture.0.home.join("parent");
        std::fs::rename(&parent, fixture.0.home.join("moved")).unwrap();
        std::fs::create_dir(&parent).unwrap();
        Ok(())
    })
    .unwrap_err();
    assert_eq!(error.code, "file_conflict");
    assert!(!fixture.0.home.join("parent/edit").exists());
    assert_eq!(
        std::fs::read(fixture.0.home.join("moved/edit")).unwrap(),
        b"before"
    );
    assert_eq!(
        std::fs::read_dir(fixture.0.home.join("moved"))
            .unwrap()
            .count(),
        1
    );
}

#[test]
fn opted_in_writes_refuse_symlinks_and_hard_links() {
    let mut fixture = Fixture::new();
    fixture.0.preserve_file_mode = true;
    let original = fixture.write("source", b"before", None).unwrap();
    let source = fixture.0.home.join("source");
    symlink(&source, fixture.0.home.join("symlink")).unwrap();
    std::fs::hard_link(&source, fixture.0.home.join("hardlink")).unwrap();
    for name in ["symlink", "hardlink"] {
        let error = fixture
            .write(name, b"after", Some(original.sha256.clone()))
            .unwrap_err();
        assert_eq!(error.code, "filesystem_rejected");
    }
    assert_eq!(std::fs::read(&source).unwrap(), b"before");
    assert_no_staging(&fixture);
}

fn mode_of(fixture: &Fixture, path: &str) -> u32 {
    std::fs::metadata(fixture.0.home.join(path))
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

fn set_test_mode(fixture: &Fixture, path: &str, mode: u32) {
    std::fs::set_permissions(
        fixture.0.home.join(path),
        std::fs::Permissions::from_mode(mode),
    )
    .unwrap();
}

fn assert_no_staging(fixture: &Fixture) {
    let entries = std::fs::read_dir(&fixture.0.home).unwrap();
    assert!(entries
        .map(|entry| entry.unwrap().file_name())
        .all(|name| !name.to_string_lossy().starts_with(".cma-write-")));
}
