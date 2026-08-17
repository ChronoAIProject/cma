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
