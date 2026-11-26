use std::io::Write as _;

use sha2::{Digest as _, Sha256};

use super::*;
use crate::command::hex;

fn config(name: &str) -> RuntimeConfig {
    let home = std::env::temp_dir().join(format!("cma-checkpoint-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let mut config = RuntimeConfig::for_test(home);
    config.checkpoint_root = config.home.with_extension(format!("{name}-objects"));
    let _ = std::fs::remove_dir_all(&config.checkpoint_root);
    std::fs::create_dir_all(&config.home).unwrap();
    config
}

fn fixture(config: &RuntimeConfig) {
    std::fs::create_dir_all(config.home.join("project/.worktrees/a/node_modules/pkg")).unwrap();
    std::fs::create_dir_all(config.home.join(".cache")).unwrap();
    std::fs::write(config.home.join("project/README.md"), "hello\n").unwrap();
    std::fs::write(
        config
            .home
            .join("project/.worktrees/a/node_modules/pkg/index.js"),
        "module.exports = 1;\n",
    )
    .unwrap();
    std::fs::write(config.home.join(".cache/disposable"), "skip").unwrap();
    std::fs::create_dir_all(config.home.join(".cma/preparing/wks-preparing")).unwrap();
    std::fs::create_dir_all(config.home.join(".cma/removing/wks-removing")).unwrap();
    std::fs::write(
        config.home.join(".cma/preparing/wks-preparing/partial"),
        "must not checkpoint",
    )
    .unwrap();
    std::fs::write(
        config.home.join(".cma/removing/wks-removing/tombstone"),
        "must not checkpoint",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("README.md", config.home.join("project/readme-link")).unwrap();
}

#[test]
fn manifest_is_deterministic_and_exclusions_are_exact_paths() {
    let config = config("manifest");
    fixture(&config);
    let manifest = crate::checkpoint_manifest::collect(&config.home, &[".cache".into()]).unwrap();
    let paths = manifest
        .entries
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            ".cma",
            "project",
            "project/.worktrees",
            "project/.worktrees/a",
            "project/.worktrees/a/node_modules",
            "project/.worktrees/a/node_modules/pkg",
            "project/.worktrees/a/node_modules/pkg/index.js",
            "project/README.md",
            #[cfg(unix)]
            "project/readme-link",
        ]
    );
    let json = serde_json::to_string(&manifest).unwrap();
    assert!(json.starts_with("{\"version\":1,\"entries\":[{\"path\":\".cma\""));
    assert!(!json.contains("disposable"));
    assert!(!json.contains("wks-preparing"));
    assert!(!json.contains("wks-removing"));
    assert!(json.contains("node_modules"));
    std::fs::remove_dir_all(config.home).unwrap();
    let _ = std::fs::remove_dir_all(config.checkpoint_root);
}

#[test]
fn restore_validation_refuses_runtime_workspace_transition_paths() {
    use crate::checkpoint_manifest::{EntryKind, Manifest, ManifestEntry};

    let config = config("transient-restore");
    for path in [".cma/preparing", ".cma/removing/wks-removing"] {
        let manifest = Manifest {
            version: crate::checkpoint_manifest::FORMAT_VERSION,
            entries: vec![ManifestEntry {
                path: path.into(),
                kind: EntryKind::Directory,
                mode: 0o700,
                size: 0,
                sha256: None,
                target: None,
                offset: None,
            }],
        };
        let error = crate::checkpoint_manifest::validate(&manifest, &config.home, 0).unwrap_err();
        assert_eq!(error.code, "checkpoint_invalid");
        assert!(error.detail.contains("transition state"));
    }
    std::fs::remove_dir_all(config.home).unwrap();
    let _ = std::fs::remove_dir_all(config.checkpoint_root);
}

#[cfg(unix)]
#[tokio::test]
async fn excluded_executable_aliases_do_not_discard_agent_history_or_allow_other_escapes() {
    let config = config("transient-aliases");
    let aliases = [".provider/tmp/aliases", ".cma/agents/a/home/tmp/aliases"];
    for path in aliases {
        std::fs::create_dir_all(config.home.join(path)).unwrap();
        std::os::unix::fs::symlink(
            "/usr/local/bin/provider",
            config.home.join(path).join("exec"),
        )
        .unwrap();
    }
    let history = config.home.join(".cma/agents/a/home/transcript.jsonl");
    std::fs::write(&history, "preserved conversation\n").unwrap();
    let rejected = crate::checkpoint_manifest::collect(&config.home, &[]).unwrap_err();
    assert_eq!(rejected.code, "checkpoint_symlink_escape");
    let store = CheckpointStore::new(&config);
    let receipt = store
        .capture(&CheckpointRequest {
            epoch: 1,
            exclusions: aliases.map(str::to_string).to_vec(),
        })
        .await
        .unwrap();
    store
        .restore(&RestoreRequest {
            object_reference: receipt.object_reference,
            sha256: receipt.sha256,
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(&history).unwrap(),
        "preserved conversation\n"
    );
    for path in aliases {
        assert!(!config.home.join(path).exists());
    }
    std::os::unix::fs::symlink("/etc/passwd", config.home.join("outside")).unwrap();
    let rejected = crate::checkpoint_manifest::collect(&config.home, &aliases.map(str::to_string))
        .unwrap_err();
    assert_eq!(rejected.code, "checkpoint_symlink_escape");
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(config.checkpoint_root).unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn codex_checkpoint_preserves_retained_home_across_generated_alias_layouts() {
    let config = config("codex-versioned-aliases");
    let wire_exclusions =
        layer3_adapter::codex::checkpoint_exclusions(&[layer3_adapter::AgentSlot::new("a")])
            .unwrap();
    assert_eq!(
        wire_exclusions,
        [".cma/agents/a/home/tmp/arg0", ".codex/tmp/arg0"]
    );
    let alias_roots = [
        ".codex/tmp/arg0",
        ".codex/tmp/arg0-linux",
        ".cma/agents/a/home/tmp/arg0",
        ".cma/agents/a/home/tmp/arg0-linux",
    ];
    for root in alias_roots {
        let directory = config.home.join(root);
        std::fs::create_dir_all(&directory).unwrap();
        for name in ["apply_patch", "applypatch", "codex-linux-sandbox"] {
            std::os::unix::fs::symlink("/usr/local/bin/provider", directory.join(name)).unwrap();
        }
    }
    let retained = [
        ".cma/agents/a/home/transcript.jsonl",
        ".cma/agents/a/home/tmp/arg0-linux-sibling/other",
        ".codex/tmp/arg0-sibling/other",
        "project/tmp/arg0-linux/other",
    ];
    for path in retained {
        let path = config.home.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "preserved fixture").unwrap();
    }
    let store = CheckpointStore::new(&config);
    let request = CheckpointRequest {
        epoch: 1,
        exclusions: wire_exclusions,
    };
    let receipt = store.capture(&request).await.unwrap();
    assert_eq!(store.capture(&request).await.unwrap(), receipt);
    store
        .restore(&RestoreRequest {
            object_reference: receipt.object_reference,
            sha256: receipt.sha256,
        })
        .await
        .unwrap();
    for path in alias_roots {
        assert!(!config.home.join(path).exists());
    }
    for path in retained {
        assert_eq!(
            std::fs::read_to_string(config.home.join(path)).unwrap(),
            "preserved fixture"
        );
    }
    std::os::unix::fs::symlink(
        "/usr/local/bin/provider",
        config.home.join("project/tmp/arg0-linux/escape"),
    )
    .unwrap();
    assert_eq!(
        store.capture(&request).await.unwrap_err().code,
        "checkpoint_symlink_escape"
    );
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(config.checkpoint_root).unwrap();
}

#[tokio::test]
async fn whole_home_round_trips_files_modes_links_and_worktree_dependencies() {
    let config = config("round-trip");
    fixture(&config);
    let store = CheckpointStore::new(&config);
    let receipt = store
        .capture(&CheckpointRequest {
            epoch: 7,
            exclusions: vec![".cache".into()],
        })
        .await
        .unwrap();
    std::fs::write(config.home.join("project/README.md"), "changed\n").unwrap();
    std::fs::write(config.home.join("new"), "remove me").unwrap();
    store
        .restore(&RestoreRequest {
            object_reference: receipt.object_reference,
            sha256: receipt.sha256,
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(config.home.join("project/README.md")).unwrap(),
        "hello\n"
    );
    assert!(config
        .home
        .join("project/.worktrees/a/node_modules/pkg/index.js")
        .is_file());
    assert!(!config.home.join("new").exists());
    assert!(!config.home.join(".cache").exists());
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(config.checkpoint_root).unwrap();
}

#[test]
fn a_truncated_archive_is_refused_even_with_its_own_digest() {
    let config = config("truncated");
    fixture(&config);
    let sealed =
        crate::checkpoint_archive::seal(&config.home, &config.checkpoint_root, &[]).unwrap();
    let mut bytes = std::fs::read(&sealed.path).unwrap();
    bytes.truncate(bytes.len() - 3);
    let truncated = config.checkpoint_root.join("truncated.cma");
    let mut file = std::fs::File::create(&truncated).unwrap();
    file.write_all(&bytes).unwrap();
    let digest = hex(&Sha256::digest(&bytes));
    let error = crate::checkpoint_archive::inspect(&truncated, &digest, &config.home).unwrap_err();
    assert_eq!(error.code, "checkpoint_invalid");
    assert!(config.home.join("project/README.md").is_file());
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(config.checkpoint_root).unwrap();
}

#[test]
fn archive_metadata_budget_covers_large_homes_but_remains_bounded() {
    let config = config("metadata-budget");
    std::fs::create_dir_all(&config.checkpoint_root).unwrap();
    let archive = config.checkpoint_root.join("header-only.cma");
    for (declared_size, expected_detail) in [
        (64 * 1024 * 1024 + 1_u64, "truncated before its payload"),
        (128 * 1024 * 1024, "truncated before its payload"),
        (128 * 1024 * 1024 + 1, "manifest length is invalid"),
    ] {
        let mut bytes = b"CMAHOMEARCHIVE1\n".to_vec();
        bytes.extend_from_slice(&declared_size.to_be_bytes());
        std::fs::write(&archive, &bytes).unwrap();
        let digest = hex(&Sha256::digest(&bytes));
        let error =
            crate::checkpoint_archive::inspect(&archive, &digest, &config.home).unwrap_err();
        assert_eq!(error.code, "checkpoint_invalid");
        assert!(error.detail.contains(expected_detail), "{error:?}");
    }
    assert_eq!(
        crate::checkpoint_archive::MAX_ARCHIVE_BYTES,
        layer3_adapter::hosted::MAX_CHECKPOINT_ARCHIVE_BYTES
    );
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(config.checkpoint_root).unwrap();
}

#[cfg(unix)]
#[test]
fn checkpoint_download_refuses_a_symlinked_object() {
    let config = config("symlink-object");
    let store = CheckpointStore::new(&config);
    std::fs::create_dir_all(&config.checkpoint_root).unwrap();
    let outside = config.home.join("outside");
    std::fs::write(&outside, "must not be served").unwrap();
    let reference = format!("home-{}.cma", "0".repeat(64));
    std::os::unix::fs::symlink(&outside, config.checkpoint_root.join(&reference)).unwrap();
    let error = store.download_path(&reference).unwrap_err();
    assert_eq!(error.code, "checkpoint_object_invalid");
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(config.checkpoint_root).unwrap();
}

#[tokio::test]
async fn finished_transfer_removal_is_exact_idempotent_and_refuses_redirects() {
    let config = config("finished-transfer");
    let store = CheckpointStore::new(&config);
    std::fs::write(config.home.join("work"), b"first snapshot").unwrap();
    let first = store
        .capture(&CheckpointRequest {
            epoch: 1,
            exclusions: Vec::new(),
        })
        .await
        .unwrap();
    std::fs::write(config.home.join("work"), b"newer snapshot").unwrap();
    let second = store
        .capture(&CheckpointRequest {
            epoch: 2,
            exclusions: Vec::new(),
        })
        .await
        .unwrap();
    let first_path = store.download_path(&first.object_reference).unwrap();
    let second_path = store.download_path(&second.object_reference).unwrap();
    store.discard(&first.object_reference).await.unwrap();
    store.discard(&first.object_reference).await.unwrap();
    assert!(!first_path.exists());
    assert_eq!(
        crate::checkpoint_manifest::sha256_file(&second_path).unwrap(),
        second.sha256
    );
    assert!(store.discard("../work").await.is_err());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(config.home.join("work"), &first_path).unwrap();
        assert_eq!(
            store
                .discard(&first.object_reference)
                .await
                .unwrap_err()
                .code,
            "checkpoint_object_invalid"
        );
        assert!(std::fs::symlink_metadata(&first_path)
            .unwrap()
            .file_type()
            .is_symlink());
    }
    assert_eq!(
        std::fs::read(config.home.join("work")).unwrap(),
        b"newer snapshot"
    );
    std::fs::remove_dir_all(config.home).unwrap();
    std::fs::remove_dir_all(config.checkpoint_root).unwrap();
}
