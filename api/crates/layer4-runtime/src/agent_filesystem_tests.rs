use super::*;
use std::os::unix::fs::{symlink, PermissionsExt as _};

#[cfg(target_os = "linux")]
#[path = "agent_filesystem/file_mode_tests.rs"]
mod file_mode;

#[cfg(not(target_os = "linux"))]
#[test]
fn a_platform_without_verified_acl_inspection_refuses_to_replace() {
    let mut fixture = Fixture::new();
    fixture.0.preserve_file_mode = true;
    let original = fixture.write("edit", b"before", None).unwrap();
    let error = fixture
        .write("edit", b"after", Some(original.sha256))
        .unwrap_err();
    assert_eq!(error.detail, "cannot preserve existing file access policy");
    assert_eq!(
        fixture.read("edit", 20).unwrap().content().unwrap(),
        b"before"
    );
}

struct Fixture(RuntimeConfig);
impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("cma-files-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&home).unwrap();
        Self(RuntimeConfig::for_test(home))
    }
    fn write(
        &self,
        path: &str,
        bytes: &[u8],
        expected: Option<String>,
    ) -> Result<AgentFileWriteReceipt> {
        write(&self.0, self.request(path, bytes, expected))
    }
    fn request(&self, path: &str, bytes: &[u8], expected: Option<String>) -> AgentFileWriteRequest {
        AgentFileWriteRequest {
            owner_access: true,
            protected_paths: Vec::new(),
            sandbox_id: self.0.sandbox_id.clone(),
            runtime_generation: self.0.generation,
            boot_id: "a".repeat(32),
            path: path.into(),
            expected_sha256: expected,
            content_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            directory: false,
        }
    }
    fn read(&self, path: &str, max_bytes: u64) -> Result<AgentFileReadReceipt> {
        read(
            &self.0,
            AgentFileReadRequest {
                owner_access: true,
                protected_paths: Vec::new(),
                sandbox_id: self.0.sandbox_id.clone(),
                runtime_generation: self.0.generation,
                boot_id: "a".repeat(32),
                home_identity: None,
                path: path.into(),
                max_bytes,
            },
        )
    }
    fn list(&self, path: &str) -> Result<AgentFileListReceipt> {
        list(
            &self.0,
            AgentFileListRequest {
                owner_access: true,
                protected_paths: Vec::new(),
                sandbox_id: self.0.sandbox_id.clone(),
                runtime_generation: self.0.generation,
                boot_id: "a".repeat(32),
                path: path.into(),
            },
        )
    }
    fn delete(&self, path: &str, recursive: bool) -> Result<AgentFileDeleteReceipt> {
        delete(
            &self.0,
            AgentFileDeleteRequest {
                owner_access: true,
                protected_paths: Vec::new(),
                sandbox_id: self.0.sandbox_id.clone(),
                runtime_generation: self.0.generation,
                boot_id: "a".repeat(32),
                path: path.into(),
                recursive,
            },
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0.home);
    }
}

#[test]
fn hidden_binary_files_round_trip_and_save_requires_current_version() {
    let fixture = Fixture::new();
    let original = fixture
        .write(".codex/session.dat", &[0, 255, 37], None)
        .unwrap();
    assert_eq!(fixture.list("").unwrap().entries[0].name, ".codex");
    assert!(fixture.list("").unwrap().entries[0].hidden);
    let read = fixture.read(".codex/session.dat", 1024).unwrap();
    assert_eq!(read.content().unwrap(), [0, 255, 37]);
    assert_eq!(read.sha256, original.sha256);
    assert_eq!(
        fixture
            .write(".codex/session.dat", b"overwrite", None)
            .unwrap_err()
            .code,
        "file_conflict"
    );
    let changed = fixture
        .write(".codex/session.dat", b"new", Some(original.sha256.clone()))
        .unwrap();
    assert_eq!(
        fixture
            .write(".codex/session.dat", b"stale", Some(original.sha256))
            .unwrap_err()
            .code,
        "file_conflict"
    );
    assert_eq!(
        fixture.read(".codex/session.dat", 1024).unwrap().sha256,
        changed.sha256
    );
    assert_eq!(
        fixture
            .write("missing", b"x", Some(changed.sha256))
            .unwrap_err()
            .code,
        "file_conflict"
    );
}

#[test]
fn traversal_symlinks_and_hard_links_never_reveal_or_modify_outside_files() {
    let fixture = Fixture::new();
    let outside = Fixture::new();
    outside.write("secret", b"private", None).unwrap();
    symlink(&outside.0.home, fixture.0.home.join("escape")).unwrap();
    symlink(
        outside.0.home.join("secret"),
        fixture.0.home.join("secret-link"),
    )
    .unwrap();
    std::fs::hard_link(
        outside.0.home.join("secret"),
        fixture.0.home.join("hard-link"),
    )
    .unwrap();
    assert_eq!(
        fixture
            .list("")
            .unwrap()
            .entries
            .iter()
            .filter(|e| e.kind == AgentFileKind::Symlink)
            .count(),
        2
    );
    for path in [
        "/etc/passwd",
        "../secret",
        "escape/secret",
        "secret-link",
        "hard-link",
    ] {
        assert!(fixture.read(path, 1024).is_err(), "{path}");
        assert!(fixture.write(path, b"changed", None).is_err(), "{path}");
    }
    assert!(fixture.list("escape").is_err());
    fixture.delete("secret-link", false).unwrap();
    fixture.delete("escape", true).unwrap();
    assert_eq!(
        std::fs::read(outside.0.home.join("secret")).unwrap(),
        b"private"
    );
    assert!(fixture.delete("", true).is_err());
}

#[test]
fn large_files_are_bounded_and_directory_lists_do_not_read_content() {
    let fixture = Fixture::new();
    let file = File::create(fixture.0.home.join("large.bin")).unwrap();
    file.set_len(MAX_AGENT_FILE_BYTES + 1).unwrap();
    let list = fixture.list("").unwrap();
    assert_eq!(list.entries[0].size_bytes, MAX_AGENT_FILE_BYTES + 1);
    assert!(list.entries[0].sha256.is_none());
    assert_eq!(
        fixture
            .read("large.bin", MAX_AGENT_FILE_BYTES)
            .unwrap_err()
            .code,
        "file_too_large"
    );
    assert!(fixture.read("large.bin", MAX_AGENT_FILE_BYTES + 1).is_err());
}

#[test]
fn directories_create_delete_and_executable_permission_survives_edit() {
    let fixture = Fixture::new();
    let request = AgentFileWriteRequest {
        owner_access: true,
        protected_paths: Vec::new(),
        sandbox_id: fixture.0.sandbox_id.clone(),
        runtime_generation: fixture.0.generation,
        boot_id: "a".repeat(32),
        path: "new/empty".into(),
        expected_sha256: None,
        content_base64: String::new(),
        directory: true,
    };
    write(&fixture.0, request.clone()).unwrap();
    write(&fixture.0, request).unwrap();
    assert!(fixture.0.home.join("new/empty").is_dir());
    let first = fixture.write("new/run.sh", b"echo first", None).unwrap();
    std::fs::set_permissions(
        fixture.0.home.join("new/run.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    fixture
        .write("new/run.sh", b"echo second", Some(first.sha256))
        .unwrap();
    assert_eq!(
        std::fs::metadata(fixture.0.home.join("new/run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert!(fixture.delete("new", false).is_err());
    fixture.delete("new", true).unwrap();
    assert!(!fixture.0.home.join("new").exists());
}

#[test]
fn repeated_symlink_swaps_cannot_escape_descriptor_confinement() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let fixture = Fixture::new();
    let outside = Fixture::new();
    outside.write("secret", b"outside", None).unwrap();
    fixture.write("safe/secret", b"inside", None).unwrap();
    let root = fixture.0.home.clone();
    let outside_root = outside.0.home.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            let _ = std::fs::rename(root.join("safe"), root.join("parked"));
            let _ = symlink(&outside_root, root.join("safe"));
            let _ = std::fs::remove_file(root.join("safe"));
            let _ = std::fs::rename(root.join("parked"), root.join("safe"));
        }
    });
    for _ in 0..200 {
        if let Ok(read) = fixture.read("safe/secret", 1024) {
            assert_eq!(read.content().unwrap(), b"inside");
        }
    }
    stop.store(true, Ordering::Relaxed);
    worker.join().unwrap();
}

#[test]
fn shared_files_hide_control_credentials_and_refuse_ancestor_deletion() {
    let fixture = Fixture::new();
    fixture
        .write("cma/.cma/provider/identity.json", b"private custody", None)
        .unwrap();
    fixture
        .write("cma/.provider/auth.json", b"private provider", None)
        .unwrap();
    fixture
        .write("cma/.notes", b"shared dotfile", None)
        .unwrap();
    let protected_paths = vec!["cma/.provider/auth.json".to_string()];
    let listing = list(
        &fixture.0,
        AgentFileListRequest {
            owner_access: false,
            protected_paths: protected_paths.clone(),
            sandbox_id: fixture.0.sandbox_id.clone(),
            runtime_generation: fixture.0.generation,
            boot_id: "a".repeat(32),
            path: "cma".into(),
        },
    )
    .unwrap();
    assert!(listing.entries.iter().any(|entry| entry.name == ".notes"));
    assert!(!listing.entries.iter().any(|entry| entry.name == ".cma"));
    for path in ["cma/.cma/provider/identity.json", "cma/.provider/auth.json"] {
        assert!(read(
            &fixture.0,
            AgentFileReadRequest {
                owner_access: false,
                protected_paths: protected_paths.clone(),
                sandbox_id: fixture.0.sandbox_id.clone(),
                runtime_generation: fixture.0.generation,
                boot_id: "a".repeat(32),
                home_identity: None,
                path: path.into(),
                max_bytes: 1024,
            }
        )
        .is_err());
        assert!(write(
            &fixture.0,
            AgentFileWriteRequest {
                owner_access: false,
                protected_paths: protected_paths.clone(),
                sandbox_id: fixture.0.sandbox_id.clone(),
                runtime_generation: fixture.0.generation,
                boot_id: "a".repeat(32),
                path: path.into(),
                content_base64: String::new(),
                expected_sha256: None,
                directory: false,
            }
        )
        .is_err());
    }
    assert!(delete(
        &fixture.0,
        AgentFileDeleteRequest {
            owner_access: false,
            protected_paths,
            sandbox_id: fixture.0.sandbox_id.clone(),
            runtime_generation: fixture.0.generation,
            boot_id: "a".repeat(32),
            path: "cma".into(),
            recursive: true,
        }
    )
    .is_err());
    assert_eq!(
        fixture
            .read("cma/.cma/provider/identity.json", 1024)
            .unwrap()
            .content()
            .unwrap(),
        b"private custody"
    );
}
