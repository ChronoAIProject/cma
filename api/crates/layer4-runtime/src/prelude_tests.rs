//! What gets written before a kernel starts, and what does not.

use super::*;

/// A home directory of this test's own.
struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("chrono-prelude-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("a temporary home");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `f` with these two variables set, and nothing else changed.
///
/// Serialized against every other test that does the same, because the
/// environment is one thing shared by the whole process.
fn with(path: Option<&str>, body: Option<&str>, f: impl FnOnce()) {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // SAFETY: the lock above is what makes this sound — no other test in this
    // process reads or writes these two names concurrently.
    unsafe {
        match path {
            Some(v) => std::env::set_var(PATH_VAR, v),
            None => std::env::remove_var(PATH_VAR),
        }
        match body {
            Some(v) => std::env::set_var(BODY_VAR, v),
            None => std::env::remove_var(BODY_VAR),
        }
    }
    f();
    unsafe {
        std::env::remove_var(PATH_VAR);
        std::env::remove_var(BODY_VAR);
    }
}

#[test]
fn a_kernel_that_needs_a_file_gets_exactly_what_it_was_given() {
    // Byte for byte, and unread. What a kernel is configured with is the
    // adapter's business; this writes it down.
    let home = Home::new("writes");
    let body = "model = \"a-model\"\n[providers.chrono]\nbase_url = \"http://127.0.0.1:9091\"\n";
    with(Some(".akernel/config.toml"), Some(body), || {
        let written = write(home.path())
            .expect("writable")
            .expect("a path was asked for");
        assert_eq!(written, home.path().join(".akernel/config.toml"));
        assert_eq!(
            std::fs::read_to_string(&written).expect("readable"),
            body,
            "the content must not be reshaped on the way to disk"
        );
    });
}

#[test]
fn a_kernel_that_needs_nothing_has_nothing_written() {
    // Two of three kernels are configured entirely by environment. Writing one
    // kernel's file into their pods is what this replaced.
    let home = Home::new("nothing");
    with(None, None, || {
        assert!(write(home.path()).expect("readable").is_none());
    });
    assert_eq!(
        std::fs::read_dir(home.path()).expect("readable").count(),
        0,
        "nothing may be created for a kernel that asked for nothing"
    );
}

#[test]
fn a_path_with_no_body_writes_nothing_rather_than_an_empty_file() {
    // An empty file where a configuration should be is worse than none: the
    // agent reads it, finds nothing, and runs on its own defaults.
    let home = Home::new("half");
    with(Some(".akernel/config.toml"), None, || {
        assert!(write(home.path()).expect("readable").is_none());
    });
}

#[test]
fn a_path_that_leaves_the_pods_home_is_refused_rather_than_written() {
    let home = Home::new("escape");
    for escape in ["/etc/passwd", "../../etc/passwd", "a/../../b", "", "."] {
        with(Some(escape), Some("body"), || {
            assert!(
                write(home.path()).is_err() || escape.is_empty(),
                "{escape} must not be written"
            );
        });
    }
}

#[test]
fn a_path_that_merely_says_here_is_kept() {
    let home = Home::new("curdir");
    with(Some("./.akernel/config.toml"), Some("body"), || {
        let written = write(home.path()).expect("writable").expect("written");
        assert_eq!(written, home.path().join(".akernel/config.toml"));
    });
}

#[test]
fn the_directory_it_belongs_in_is_created() {
    // A fresh pod has nothing under its home but the home itself.
    let home = Home::new("mkdir");
    with(Some("deep/er/still/config.toml"), Some("body"), || {
        assert!(write(home.path()).expect("writable").is_some());
    });
    assert!(home.path().join("deep/er/still/config.toml").exists());
}

#[cfg(unix)]
#[test]
fn a_kernel_config_cannot_cross_an_intermediate_symlink() {
    let home = Home::new("config-intermediate-link");
    let outside = Home::new("config-intermediate-outside");
    std::os::unix::fs::symlink(outside.path(), home.path().join(".akernel"))
        .expect("the intermediate escape is created");

    with(Some(".akernel/config.toml"), Some("private"), || {
        write(home.path()).expect_err("an intermediate symlink must be refused");
    });
    assert!(
        !outside.path().join("config.toml").exists(),
        "the configuration must not land beyond the runtime home"
    );
}

#[cfg(unix)]
#[test]
fn a_kernel_config_cannot_cross_a_final_file_symlink() {
    let home = Home::new("config-final-link");
    let outside = Home::new("config-final-outside");
    std::fs::create_dir_all(home.path().join(".akernel")).expect("the target directory exists");
    let outside_file = outside.path().join("kept.toml");
    std::fs::write(&outside_file, "unchanged").expect("the outside file exists");
    std::os::unix::fs::symlink(&outside_file, home.path().join(".akernel/config.toml"))
        .expect("the final escape is created");

    with(Some(".akernel/config.toml"), Some("private"), || {
        write(home.path()).expect_err("a final symlink must be refused");
    });
    assert_eq!(
        std::fs::read_to_string(outside_file).expect("the outside file remains readable"),
        "unchanged"
    );
}

/// Carrying many files in one body.
///
/// The framing exists because the environment cannot hold these: one skill's
/// reference alone is larger than a single environment string may be, and a
/// pod that exceeds it fails to start with nothing naming the cause.
mod a_pack_of_files {
    use super::*;

    /// The framing, written the way the control plane writes it.
    fn pack(files: &[(&str, &[u8])]) -> Vec<u8> {
        let header = serde_json::json!({
            "chronoPrelude": 1,
            "files": files
                .iter()
                .map(|(path, body)| serde_json::json!({ "path": path, "len": body.len() }))
                .collect::<Vec<_>>(),
        });
        let mut out = header.to_string().into_bytes();
        out.push(b'\n');
        for (_, body) in files {
            out.extend_from_slice(body);
        }
        out
    }

    #[test]
    fn carries_every_file_in_the_order_it_was_declared() {
        let got = unpack(&pack(&[
            (".codex/skills/nyxid/SKILL.md", b"the manual"),
            (".codex/skills/nyxid/references/api.md", b"the reference"),
        ]))
        .expect("a well-formed pack");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].0, PathBuf::from(".codex/skills/nyxid/SKILL.md"));
        assert_eq!(got[0].1, b"the manual");
        assert_eq!(
            got[1].0,
            PathBuf::from(".codex/skills/nyxid/references/api.md")
        );
        assert_eq!(got[1].1, b"the reference");
    }

    #[test]
    fn refuses_a_path_that_would_leave_the_pod_s_home() {
        // The bodies crossed an HTTP hop. A path taken on trust is a write
        // somewhere this process should not be writing.
        for escape in [
            "../../etc/passwd",
            "/etc/passwd",
            ".codex/../../../root/.ssh/authorized_keys",
        ] {
            let refused =
                unpack(&pack(&[(escape, b"x")])).expect_err("{escape} must not be written");
            assert!(refused.contains(escape), "the refusal names it: {refused}");
        }
    }

    #[test]
    fn refuses_a_length_longer_than_the_body_carries() {
        // Declared lengths are read before slicing, so a pack claiming more
        // than it holds is refused rather than panicking.
        let mut body = serde_json::json!({
            "chronoPrelude": 1,
            "files": [{ "path": "a.md", "len": 9_999 }],
        })
        .to_string()
        .into_bytes();
        body.push(b'\n');
        body.extend_from_slice(b"short");
        let refused = unpack(&body).expect_err("a truncated pack is refused");
        assert!(refused.contains("a.md"), "got {refused}");
    }

    #[test]
    fn refuses_noncanonical_duplicate_and_control_character_targets() {
        let duplicate = unpack(&pack(&[("a.md", b"one"), ("a.md", b"two")]))
            .expect_err("one target may not be written twice");
        assert!(duplicate.contains("repeats"), "got {duplicate}");

        for path in ["./a.md", "dir/./a.md", "bad\u{1}.md"] {
            let refused = unpack(&pack(&[(path, b"body")]))
                .expect_err("the runtime accepts only canonical targets");
            assert!(refused.contains(path), "got {refused}");
        }
    }

    #[test]
    fn refuses_more_than_the_sender_s_file_limit() {
        let entries = (0..=PACK_FILE_LIMIT)
            .map(|index| serde_json::json!({ "path": format!("file-{index}"), "len": 0 }))
            .collect::<Vec<_>>();
        let mut body = serde_json::to_vec(&serde_json::json!({
            "chronoPrelude": PACK_VERSION,
            "files": entries,
        }))
        .expect("the oversized header serializes");
        body.push(b'\n');

        let refused = unpack(&body).expect_err("the file count is bounded while decoding");
        assert!(refused.contains("too many files"), "got {refused}");
    }

    #[test]
    fn refuses_unclaimed_trailing_bytes() {
        let mut body = pack(&[("a.md", b"body")]);
        body.push(b'x');
        let refused = unpack(&body).expect_err("every byte must belong to one declared file");
        assert!(refused.contains("trailing"), "got {refused}");
    }

    #[test]
    fn refuses_a_framing_it_does_not_recognise() {
        let mut body = serde_json::json!({ "somethingElse": 1, "files": [] })
            .to_string()
            .into_bytes();
        body.push(b'\n');
        assert!(unpack(&body).is_err(), "an unknown framing is refused");
        assert!(
            unpack(b"no header line").is_err(),
            "a bodiless pack is refused"
        );
    }

    #[test]
    fn refuses_a_pack_larger_than_the_cap_before_reading_it() {
        let refused = unpack(&vec![b'x'; PACK_LIMIT + 1]).expect_err("over the cap");
        assert!(refused.contains(&PACK_LIMIT.to_string()), "got {refused}");
    }

    #[test]
    fn writes_what_it_was_handed_creating_directories_on_the_way() {
        let home = Home::new("pack-write");
        let files = unpack(&pack(&[(".codex/skills/x/SKILL.md", b"body")])).expect("a pack");
        let written = write_all(home.path(), &files).expect("written");
        assert_eq!(written, 1);
        let landed = std::fs::read_to_string(home.path().join(".codex/skills/x/SKILL.md"))
            .expect("the file is there");
        assert_eq!(landed, "body");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_an_intermediate_directory_symlink_without_writing_beyond_home() {
        let home = Home::new("pack-intermediate-link");
        let outside = Home::new("pack-intermediate-outside");
        std::os::unix::fs::symlink(outside.path(), home.path().join(".codex"))
            .expect("the intermediate escape is created");
        let files = unpack(&pack(&[(".codex/skills/x/SKILL.md", b"private")]))
            .expect("the pack itself is well formed");

        write_all(home.path(), &files).expect_err("an intermediate symlink must be refused");
        assert!(
            !outside.path().join("skills/x/SKILL.md").exists(),
            "skill bytes must not land beyond the runtime home"
        );
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_final_file_symlink_without_overwriting_its_target() {
        let home = Home::new("pack-final-link");
        let outside = Home::new("pack-final-outside");
        let target_dir = home.path().join(".codex/skills/x");
        std::fs::create_dir_all(&target_dir).expect("the target directory exists");
        let outside_file = outside.path().join("kept.md");
        std::fs::write(&outside_file, "unchanged").expect("the outside file exists");
        std::os::unix::fs::symlink(&outside_file, target_dir.join("SKILL.md"))
            .expect("the final escape is created");
        let files = unpack(&pack(&[(".codex/skills/x/SKILL.md", b"private")]))
            .expect("the pack itself is well formed");

        write_all(home.path(), &files).expect_err("a final symlink must be refused");
        assert_eq!(
            std::fs::read_to_string(outside_file).expect("the outside file remains readable"),
            "unchanged"
        );
    }

    #[cfg(unix)]
    #[test]
    fn replaces_a_final_hardlink_without_overwriting_its_other_name() {
        let home = Home::new("pack-final-hardlink");
        let outside = Home::new("pack-final-hardlink-outside");
        let target_dir = home.path().join(".codex/skills/x");
        std::fs::create_dir_all(&target_dir).expect("the target directory exists");
        let outside_file = outside.path().join("kept.md");
        std::fs::write(&outside_file, "unchanged").expect("the outside file exists");
        let target = target_dir.join("SKILL.md");
        std::fs::hard_link(&outside_file, &target).expect("the final hardlink is created");
        let files = unpack(&pack(&[(".codex/skills/x/SKILL.md", b"private")]))
            .expect("the pack itself is well formed");

        assert_eq!(
            write_all(home.path(), &files).expect("the target is replaced"),
            1
        );
        assert_eq!(
            std::fs::read_to_string(&outside_file).expect("the outside name remains readable"),
            "unchanged"
        );
        assert_eq!(
            std::fs::read_to_string(target).expect("the runtime target was overwritten"),
            "private"
        );
    }
}
