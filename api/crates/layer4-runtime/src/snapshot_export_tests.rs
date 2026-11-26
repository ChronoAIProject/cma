use layer3_adapter::{
    HomeSnapshotConversationIdentity, HomeSnapshotObservation, HomeSnapshotReceipt,
    HomeSnapshotRequest, HomeVolumeAttachment, HomeVolumePolicy, HomeVolumeReceipt,
    HomeVolumeRequest, InstallReceipt, InstallRequest,
};
use sha2::{Digest, Sha256};

use super::*;

struct Fixture {
    root: PathBuf,
    source: ReadOnlySource,
    request: HomeSnapshotExportRequest,
    output: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("cma-export-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&root).unwrap();
        let root = std::fs::canonicalize(root).unwrap();
        let home = HomeVolumeRequest {
            scope: None,
            volume_id: "volume-one".into(),
            sandbox_id: "sandbox-test".into(),
            policy: HomeVolumePolicy {
                mode: Default::default(),
                storage_class: "cma-home-v1".into(),
                capacity_gib: 64,
                template_name: "seed".into(),
                template_generation: "seed-one".into(),
            },
        };
        let home = HomeVolumeAttachment {
            receipt: HomeVolumeReceipt {
                native_authority: None,
                volume_id: home.volume_id.clone(),
                request_sha256: home.computed_sha256(),
                storage_reference: "home-one".into(),
                storage_generation: "disk-one".into(),
            },
            request: home,
        };
        let snapshot = HomeSnapshotRequest {
            capture_id: "capture-one".into(),
            epoch: 1,
            home,
            snapshot_class: "cma-home-v1".into(),
        };
        let observation = HomeSnapshotObservation {
            receipt: HomeSnapshotReceipt {
                capture_id: snapshot.capture_id.clone(),
                request_sha256: snapshot.computed_sha256(),
                snapshot_reference: "snapshot-one".into(),
                snapshot_generation: "snapshot-uid-one".into(),
            },
            captured_at: Some("2026-09-10T00:00:00Z".into()),
            ready: true,
        };
        let request = HomeSnapshotExportRequest {
            snapshot,
            observation,
            runtime_generation: 1,
            conversations: vec![HomeSnapshotConversationIdentity {
                agent_id: "agt_one".into(),
                conversation_id: "conversation-one".into(),
            }],
        };
        let config = RuntimeConfig {
            home_identity: Some(request.snapshot.home.identity().unwrap()),
            ..RuntimeConfig::for_test(root.join("snapshot-home"))
        };
        std::fs::create_dir_all(config.agent_home("agt_one").join("sessions")).unwrap();
        std::fs::create_dir_all(config.home.join("project")).unwrap();
        std::fs::write(config.home.join("project/changed"), "captured content").unwrap();
        std::fs::write(
            config.home.join("project/deleted-later"),
            "captured deletion target",
        )
        .unwrap();
        std::fs::write(
            config.agent_home("agt_one").join("sessions/a-main.jsonl"),
            main_conversation(),
        )
        .unwrap();
        std::fs::write(
            config.agent_home("agt_one").join("sessions/z-child.jsonl"),
            b"{\"payload\":{\"id\":\"child-conversation\"}}\n",
        )
        .unwrap();
        crate::persistent_home::initialize(&config).unwrap();
        let install = InstallRequest::new("export-install", vec![]);
        let record = crate::install_state::InstallRecord::completed(
            &config,
            &install,
            InstallReceipt {
                operation_id: install.operation_id.clone(),
                request_sha256: install.request_sha256.clone(),
                commands: vec![],
            },
        );
        crate::install_state::write_initial(
            &config,
            &crate::install_state::InstallRecord::started(&config, &install),
        )
        .unwrap();
        crate::install_state::replace(&config, &record).unwrap();
        let output = root.join("export");
        std::fs::create_dir(&output).unwrap();
        // This is a separate fixed tree fixture. Production must obtain this
        // private type through the read-only filesystem check below.
        Self {
            root,
            source: ReadOnlySource { config },
            request,
            output,
        }
    }

    fn run(&self) -> Result<HomeSnapshotExportReceipt> {
        export(&self.source, &self.request, &self.output)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}

fn main_conversation() -> &'static [u8] {
    b"{\"payload\":{\"id\":\"conversation-one\"}}\n{\"event\":\"captured reply\"}\n"
}

#[test]
fn export_uses_one_fixed_tree_and_the_admitted_conversation() {
    let fixture = Fixture::new();
    let live = fixture.root.join("live-home");
    std::fs::create_dir_all(live.join("project")).unwrap();
    std::fs::write(live.join("project/changed"), "later live content").unwrap();
    std::fs::write(live.join("project/created-later"), "later creation").unwrap();
    // The captured source retains deleted-later; the live tree has removed it.
    let binding = fixture
        .source
        .config
        .state_root
        .join("home-binding/binding.json");
    let before = std::fs::read(&binding).unwrap();
    let receipt = fixture.run().unwrap();
    receipt.validate_for(&fixture.request).unwrap();
    assert_eq!(
        receipt.conversations[0].archive_sha256,
        format!("{:x}", Sha256::digest(main_conversation()))
    );
    let restored = fixture.root.join("restored");
    crate::checkpoint_archive::extract(
        &fixture.output.join(&receipt.checkpoint.object_reference),
        &receipt.checkpoint.sha256,
        &fixture.source.config.home,
        &restored,
    )
    .unwrap();
    assert_eq!(
        std::fs::read(restored.join("project/changed")).unwrap(),
        b"captured content"
    );
    assert!(restored.join("project/deleted-later").is_file());
    assert!(!restored.join("project/created-later").exists());
    assert_eq!(
        std::fs::read(restored.join(".cma/agents/agt_one/home/sessions/a-main.jsonl")).unwrap(),
        main_conversation()
    );
    assert_eq!(std::fs::read(binding).unwrap(), before);
    assert_eq!(
        std::fs::read(live.join("project/changed")).unwrap(),
        b"later live content"
    );
    assert_eq!(fixture.run().unwrap(), receipt);
    assert_eq!(std::fs::read_dir(&fixture.output).unwrap().count(), 3);
}

#[test]
fn read_only_mount_gate_rejects_writable_sources_before_any_export_write() {
    let fixture = Fixture::new();
    assert!(!io::read_only(&fixture.source.config.home).unwrap());
    assert!(ReadOnlySource::open(fixture.source.config.clone(), &fixture.output).is_err());
    assert_eq!(std::fs::read_dir(&fixture.output).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn projected_request_links_are_resolved_only_inside_the_mount() {
    let fixture = Fixture::new();
    let projected = fixture.root.join("projected");
    let data = projected.join("..data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("request.json"), br#"{"ok":true}"#).unwrap();
    std::os::unix::fs::symlink("..data", projected.join("current")).unwrap();
    let value: serde_json::Value = io::read_projected(&projected.join("current/request.json"))
        .unwrap()
        .unwrap();
    assert_eq!(value["ok"], true);

    let outside = fixture.root.join("outside.json");
    std::fs::write(&outside, br#"{"ok":false}"#).unwrap();
    std::os::unix::fs::symlink(&outside, projected.join("escape.json")).unwrap();
    assert!(io::read_projected::<serde_json::Value>(&projected.join("escape.json")).is_err());
}

#[test]
fn source_generation_and_installation_are_verified_without_repair() {
    let fixture = Fixture::new();
    let mut request = fixture.request.clone();
    request.runtime_generation = 2;
    assert!(export(&fixture.source, &request, &fixture.output).is_err());
    let binding = fixture
        .source
        .config
        .state_root
        .join("home-binding/binding.json");
    std::fs::write(&binding, b"corrupt").unwrap();
    assert!(fixture.run().is_err());
    assert_eq!(std::fs::read(binding).unwrap(), b"corrupt");
    assert_eq!(std::fs::read_dir(&fixture.output).unwrap().count(), 0);
}

#[test]
fn installation_authority_must_come_from_captured_transfer_storage() {
    let fixture = Fixture::new();
    let source = fixture.source.config.checkpoint_root.join("runtime-state");
    // A valid installation record in writable output is not captured evidence.
    std::fs::rename(&source, fixture.output.join("runtime-state")).unwrap();
    assert!(fixture.run().is_err());
    assert!(!fixture.output.join("export-request.json").exists());
    assert!(!source.exists());
    std::fs::rename(fixture.output.join("runtime-state"), &source).unwrap();
    fixture.run().unwrap();

    let configured = source_config(&fixture.request).unwrap();
    assert_eq!(
        configured.checkpoint_root,
        Path::new("/var/lib/cma-transfer/cma-checkpoints")
    );
}

#[test]
fn missing_or_duplicate_conversations_never_publish_an_archive() {
    for duplicate in [false, true] {
        let fixture = Fixture::new();
        let home = fixture.source.config.agent_home("agt_one");
        if duplicate {
            std::fs::write(home.join("sessions/duplicate.jsonl"), main_conversation()).unwrap();
        } else {
            std::fs::remove_file(home.join("sessions/a-main.jsonl")).unwrap();
        }
        assert!(fixture.run().is_err());
        assert!(!fixture.output.join("export.json").exists());
        assert!(std::fs::read_dir(&fixture.output)
            .unwrap()
            .all(|entry| entry
                .unwrap()
                .path()
                .extension()
                .is_none_or(|extension| extension != "cma")));
    }
}

#[test]
fn output_cannot_be_reused_by_another_capture_or_accept_corrupted_bytes() {
    let fixture = Fixture::new();
    let receipt = fixture.run().unwrap();
    let mut other = fixture.request.clone();
    other.observation.captured_at = Some("2026-09-10T00:01:00Z".into());
    assert!(export(&fixture.source, &other, &fixture.output).is_err());
    let archive = fixture.output.join(&receipt.checkpoint.object_reference);
    std::fs::write(&archive, b"corrupt").unwrap();
    assert!(fixture.run().is_err());
    assert_eq!(std::fs::read(archive).unwrap(), b"corrupt");
}

#[cfg(unix)]
#[test]
fn disposable_aliases_are_excluded_but_foreign_agent_paths_are_refused() {
    let fixture = Fixture::new();
    let alias_roots = [
        ".codex/tmp/arg0",
        ".codex/tmp/arg0-linux",
        ".cma/agents/agt_one/home/tmp/arg0",
        ".cma/agents/agt_one/home/tmp/arg0-linux",
    ];
    for path in alias_roots {
        let aliases = fixture.source.config.home.join(path);
        std::fs::create_dir_all(&aliases).unwrap();
        for name in ["apply_patch", "applypatch", "codex-linux-sandbox"] {
            std::os::unix::fs::symlink("/usr/local/bin/tool", aliases.join(name)).unwrap();
        }
    }
    let receipt = fixture.run().unwrap();
    let inspected = crate::checkpoint_archive::inspect(
        &fixture.output.join(receipt.checkpoint.object_reference),
        &receipt.checkpoint.sha256,
        &fixture.source.config.home,
    )
    .unwrap();
    assert!(inspected
        .manifest
        .entries
        .iter()
        .all(|entry| !entry.path.contains("tmp/arg0")));
    std::os::unix::fs::symlink(
        &fixture.root,
        fixture.source.config.home.join(".cma/agents/agt_foreign"),
    )
    .unwrap();
    assert!(agent_slots(&fixture.source.config.home).is_err());
}

#[test]
fn export_request_and_receipt_refuse_incomplete_or_mismatched_authority() {
    let fixture = Fixture::new();
    let receipt = fixture.run().unwrap();
    for index in 0..5 {
        let mut request = fixture.request.clone();
        match index {
            0 => request.observation.ready = false,
            1 => request.observation.captured_at = None,
            2 => request.observation.receipt.snapshot_generation.clear(),
            3 => request.conversations.push(request.conversations[0].clone()),
            _ => request.conversations[0].agent_id = "../other".into(),
        }
        assert!(request.validate().is_err());
    }
    for index in 0..4 {
        let mut changed = receipt.clone();
        match index {
            0 => changed.checkpoint.writers = WriterState::Held,
            1 => changed.request_sha256 = "a".repeat(64),
            2 => changed.conversations[0].identity.conversation_id = "other".into(),
            _ => changed.conversations[0].archive_sha256 = "invalid".into(),
        }
        assert!(changed.validate_for(&fixture.request).is_err());
    }
}

#[cfg(unix)]
#[test]
fn export_records_are_bounded_and_never_follow_symlinks() {
    let fixture = Fixture::new();
    let foreign = fixture.root.join("foreign.json");
    std::fs::write(&foreign, serde_json::to_vec(&fixture.request).unwrap()).unwrap();
    let linked = fixture.output.join("export-request.json");
    std::os::unix::fs::symlink(&foreign, &linked).unwrap();
    assert!(fixture.run().is_err());
    assert!(std::fs::symlink_metadata(linked)
        .unwrap()
        .file_type()
        .is_symlink());
    let huge = fixture.output.join("huge.json");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(256 * 1024 + 1)
        .unwrap();
    assert!(io::read::<HomeSnapshotExportRequest>(&huge).is_err());
}
