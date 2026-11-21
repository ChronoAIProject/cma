use super::*;
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("cma-attachments-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn request(&self, bytes: &[u8]) -> AgentAttachmentRequest {
        AgentAttachmentRequest {
            sandbox_id: "sbx_test".into(),
            runtime_generation: 1,
            boot_id: "a".repeat(32),
            agent_id: layer3_adapter::AgentSlot::new("agt_test"),
            attachment_id: format!("att_{}", "b".repeat(64)),
            name: "drawing.png".into(),
            media_type: "image/png".into(),
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }
    fn file(&self, request: &AgentAttachmentRequest) -> PathBuf {
        self.0
            .join("attachments")
            .join(request.agent_id.as_str())
            .join(&request.attachment_id)
            .join(&request.name)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn attachment_publication_survives_restart_and_never_overwrites_a_reused_id() {
    let fixture = Fixture::new();
    let bytes = b"persisted upload";
    let request = fixture.request(bytes);
    let receipt = publish(&fixture.0, &request, bytes).unwrap();
    receipt.validate_for(&request).unwrap();
    assert_eq!(std::fs::read(fixture.file(&request)).unwrap(), bytes);
    let mut resumed = request.clone();
    resumed.runtime_generation = 2;
    resumed.boot_id = "c".repeat(32);
    assert_eq!(publish(&fixture.0, &resumed, bytes).unwrap(), receipt);
    for case in 0..3 {
        let mut changed = resumed.clone();
        match case {
            0 => changed.name = "other.png".into(),
            1 => changed.media_type = "text/plain".into(),
            _ => {
                changed.size_bytes = 7;
                changed.sha256 = format!("{:x}", Sha256::digest(b"changed"));
            }
        }
        let content = if case == 2 {
            b"changed".as_slice()
        } else {
            bytes
        };
        assert_eq!(
            publish(&fixture.0, &changed, content).unwrap_err().code,
            "attachment_upload_conflict"
        );
        assert_eq!(std::fs::read(fixture.file(&request)).unwrap(), bytes);
    }
    let parent = fixture
        .file(&request)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    assert!(std::fs::read_dir(parent).unwrap().all(|entry| !entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".upload-")));
}

#[test]
fn uploads_reject_symlink_parents_files_and_hardlink_replays() {
    use std::os::unix::fs::symlink;
    for level in 0..3 {
        let fixture = Fixture::new();
        let outside = Fixture::new();
        let request = fixture.request(b"x");
        let parts = [
            "attachments",
            request.agent_id.as_str(),
            request.attachment_id.as_str(),
        ];
        let mut parent = fixture.0.clone();
        for part in &parts[..level] {
            parent.push(part);
            std::fs::create_dir(&parent).unwrap();
        }
        symlink(&outside.0, parent.join(parts[level])).unwrap();
        assert!(publish(&fixture.0, &request, b"x").is_err());
        assert!(std::fs::read_dir(&outside.0).unwrap().next().is_none());
    }
    for hard in [false, true] {
        let fixture = Fixture::new();
        let request = fixture.request(b"x");
        publish(&fixture.0, &request, b"x").unwrap();
        let path = fixture.file(&request);
        let original = fixture.0.join("owner-file");
        std::fs::rename(&path, &original).unwrap();
        if hard {
            std::fs::hard_link(&original, &path).unwrap();
        } else {
            symlink(&original, &path).unwrap();
        }
        assert!(publish(&fixture.0, &request, b"x").is_err());
        assert_eq!(std::fs::read(original).unwrap(), b"x");
    }
}

#[test]
fn invalid_partial_content_never_publishes_and_temporary_stage_cleans_itself() {
    let fixture = Fixture::new();
    let request = fixture.request(b"complete");
    assert!(publish(&fixture.0, &request, b"partial").is_err());
    assert!(!fixture.0.join("attachments").exists());
    let parent = directory(&fixture.0).unwrap();
    let name;
    {
        let stage = Stage::new(&parent, "file").unwrap();
        name = stage.name.clone();
        write_new(&stage.directory, "file", b"partial").unwrap();
    }
    assert!(!fixture.0.join(name).exists());
    let empty = fixture.request(b"");
    publish(&fixture.0, &empty, b"").unwrap();
    assert_eq!(std::fs::metadata(fixture.file(&empty)).unwrap().len(), 0);
}

#[test]
fn simultaneous_same_id_uploads_publish_one_complete_result() {
    let fixture = Fixture::new();
    let content = vec![123; 128 * 1024];
    let request = fixture.request(&content);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    std::thread::scope(|scope| {
        let mut tasks = vec![];
        for _ in 0..4 {
            let barrier = barrier.clone();
            let request = &request;
            let content = &content;
            let root = &fixture.0;
            tasks.push(scope.spawn(move || {
                barrier.wait();
                publish(root, request, content)
            }));
        }
        for task in tasks {
            task.join()
                .unwrap()
                .unwrap()
                .validate_for(&request)
                .unwrap();
        }
    });
    assert_eq!(std::fs::read(fixture.file(&request)).unwrap(), content);
}

#[test]
fn only_the_complete_mounted_home_advertises_uploads() {
    assert!(!mounted_home(
        "1 2 0:0 / /home/cma rw - ext4 /dev/fixture rw"
    ));
    assert!(mounted_home(
        "1 2 0:0 /home /home rw - ext4 /dev/fixture rw"
    ));
    assert!(!mounted_home(
        "1 2 0:0 / /home/attachments rw - ext4 /dev/fixture rw"
    ));
    let fixture = Fixture::new();
    assert!(!supported(&RuntimeConfig::for_test(fixture.0.clone())));
}
