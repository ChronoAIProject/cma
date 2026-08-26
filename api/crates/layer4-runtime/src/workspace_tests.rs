use std::os::unix::fs::PermissionsExt as _;

use axum::extract::{Query, State};
use axum::response::IntoResponse as _;

use super::*;

fn fixture(entries: Vec<(&str, EntryKind, u32, &[u8])>) -> (Vec<u8>, Header) {
    let mut offset = 0_u64;
    let mut payload = Vec::new();
    let entries = entries
        .into_iter()
        .map(|(path, kind, mode, body)| {
            let entry = Entry {
                path: path.into(),
                kind,
                mode,
                size: body.len() as u64,
                content_sha256: hex(&Sha256::digest(body)),
                provider_object_id: Some("1".repeat(40)),
                offset,
            };
            offset += body.len() as u64;
            payload.extend_from_slice(body);
            entry
        })
        .collect();
    let mut header = Header {
        cma_workspace: VERSION,
        workspace_epoch: 1,
        checkpoint_version: 0,
        quiesced_writers: 0,
        source_revision: "a".repeat(40),
        manifest_digest: "0".repeat(64),
        entries,
    };
    header.manifest_digest = manifest_digest(&header, &payload).expect("manifest digest");
    let encoded = serde_json::to_vec(&header).expect("header encodes");
    let mut archive = (encoded.len() as u64).to_be_bytes().to_vec();
    archive.extend_from_slice(&encoded);
    archive.extend_from_slice(&payload);
    (archive, header)
}

fn root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "chrono-workspace-test-{}-{name}",
        std::process::id()
    ))
}

#[test]
fn a_verified_archive_restores_binary_executable_and_symlink_entries() {
    let base = root("roundtrip");
    let target = base.join("workspace");
    let _ = std::fs::remove_dir_all(&base);
    let (archive, header) = fixture(vec![
        ("README.md", EntryKind::Regular, 0o100755, b"hello\n"),
        ("bin/data.bin", EntryKind::Regular, 0o100644, b"\0\xff\x01"),
        ("readme-link", EntryKind::Symlink, 0o120000, b"README.md"),
    ]);
    let archive_sha = hex(&Sha256::digest(&archive));

    let restored = restore(
        &target,
        &archive,
        1,
        &header.source_revision,
        &header.manifest_digest,
        &archive_sha,
    )
    .expect("restore succeeds");

    assert_eq!(restored.entries, 3);
    assert_eq!(
        std::fs::read(target.join("bin/data.bin")).unwrap(),
        b"\0\xff\x01"
    );
    assert_eq!(
        std::fs::metadata(target.join("README.md"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::read_link(target.join("readme-link")).unwrap(),
        PathBuf::from("README.md")
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn corrupt_or_truncated_archives_never_make_a_workspace_visible() {
    let base = root("corrupt");
    let target = base.join("workspace");
    let _ = std::fs::remove_dir_all(&base);
    let (mut archive, header) = fixture(vec![(
        "README.md",
        EntryKind::Regular,
        0o100644,
        b"hello\n",
    )]);
    archive.pop();
    let claimed_sha = hex(&Sha256::digest(&archive));

    assert!(restore(
        &target,
        &archive,
        1,
        &header.source_revision,
        &header.manifest_digest,
        &claimed_sha,
    )
    .is_err());
    assert!(!target.exists());
    let _ = std::fs::remove_dir_all(base);
}

#[test]
fn traversal_and_git_control_paths_are_rejected_before_extraction() {
    for path in ["../outside", "/absolute", ".git/config", "a/.GIT/config"] {
        let base = root(&path.replace(['/', '.'], "_"));
        let target = base.join("workspace");
        let _ = std::fs::remove_dir_all(&base);
        let (archive, header) = fixture(vec![(path, EntryKind::Regular, 0o100644, b"content")]);
        let archive_sha = hex(&Sha256::digest(&archive));
        assert!(restore(
            &target,
            &archive,
            1,
            &header.source_revision,
            &header.manifest_digest,
            &archive_sha,
        )
        .is_err());
        assert!(!target.exists());
        let _ = std::fs::remove_dir_all(base);
    }
}

#[test]
fn an_exact_repeated_restore_is_idempotent_but_a_changed_tree_is_not_overwritten() {
    let base = root("idempotent");
    let target = base.join("workspace");
    let _ = std::fs::remove_dir_all(&base);
    let (archive, header) = fixture(vec![(
        "README.md",
        EntryKind::Regular,
        0o100644,
        b"baseline\n",
    )]);
    let archive_sha = hex(&Sha256::digest(&archive));

    restore(
        &target,
        &archive,
        1,
        &header.source_revision,
        &header.manifest_digest,
        &archive_sha,
    )
    .expect("first restore succeeds");
    restore(
        &target,
        &archive,
        1,
        &header.source_revision,
        &header.manifest_digest,
        &archive_sha,
    )
    .expect("an exact retry reconciles");

    std::fs::write(target.join("README.md"), b"user work\n").unwrap();
    assert!(restore(
        &target,
        &archive,
        1,
        &header.source_revision,
        &header.manifest_digest,
        &archive_sha,
    )
    .is_err());
    assert_eq!(
        std::fs::read(target.join("README.md")).unwrap(),
        b"user work\n"
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn capture_round_trip_preserves_the_complete_current_tree() {
    use std::os::unix::fs::symlink;

    let base = root("capture-roundtrip");
    let workspace = base.join("source");
    let checkpoints = base.join("checkpoints");
    let restored = base.join("restored");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(workspace.join("bin")).unwrap();
    std::fs::write(workspace.join("README.md"), b"edited\n").unwrap();
    std::fs::write(workspace.join("bin/data.bin"), b"\0\xff\x01\x80").unwrap();
    std::fs::write(workspace.join("untracked.txt"), b"new\n").unwrap();
    std::fs::set_permissions(
        workspace.join("README.md"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink("README.md", workspace.join("readme-link")).unwrap();
    // A baseline-only path is represented as a deletion by being absent from
    // this complete-tree capture.
    assert!(!workspace.join("deleted-from-baseline.txt").exists());

    let captured = prepare_capture(&workspace, &checkpoints, 7, 1, &"a".repeat(40), 2)
        .expect("capture prepares")
        .commit()
        .expect("capture commits");
    assert_eq!(captured.entries, 4);
    assert_eq!(captured.quiesced_writers, 2);

    restore(
        &restored,
        &captured.archive,
        captured.workspace_epoch,
        &captured.source_revision,
        &captured.manifest_digest,
        &captured.archive_sha256,
    )
    .expect("captured archive restores");
    assert_eq!(
        std::fs::read(restored.join("README.md")).unwrap(),
        b"edited\n"
    );
    assert_eq!(
        std::fs::read(restored.join("bin/data.bin")).unwrap(),
        b"\0\xff\x01\x80"
    );
    assert_eq!(
        std::fs::read(restored.join("untracked.txt")).unwrap(),
        b"new\n"
    );
    assert_eq!(
        std::fs::metadata(restored.join("README.md"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    assert_eq!(
        std::fs::read_link(restored.join("readme-link")).unwrap(),
        PathBuf::from("README.md")
    );
    assert!(!restored.join("deleted-from-baseline.txt").exists());
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_checkpoint_version_is_immutable_across_ambiguous_retries() {
    let base = root("capture-idempotent");
    let workspace = base.join("source");
    let checkpoints = base.join("checkpoints");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("work.txt"), b"first\n").unwrap();

    let first = prepare_capture(&workspace, &checkpoints, 3, 9, &"b".repeat(40), 0)
        .unwrap()
        .commit()
        .unwrap();
    std::fs::write(workspace.join("work.txt"), b"second\n").unwrap();

    let retained = load_capture(&checkpoints, 3, 9, &"b".repeat(40))
        .unwrap()
        .expect("committed version remains available");
    assert_eq!(retained.archive, first.archive);
    let changed = prepare_capture(&workspace, &checkpoints, 3, 9, &"b".repeat(40), 0)
        .unwrap()
        .commit();
    assert!(changed
        .unwrap_err()
        .contains("already names different workspace bytes"));
    assert_eq!(
        load_capture(&checkpoints, 3, 9, &"b".repeat(40))
            .unwrap()
            .unwrap()
            .archive,
        first.archive
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn uncommitted_or_corrupt_checkpoint_files_are_never_authoritative() {
    let base = root("capture-commit-order");
    let workspace = base.join("source");
    let checkpoints = base.join("checkpoints");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("work.txt"), b"complete\n").unwrap();

    let prepared = prepare_capture(&workspace, &checkpoints, 4, 1, &"c".repeat(40), 0).unwrap();
    assert!(load_capture(&checkpoints, 4, 1, &"c".repeat(40))
        .unwrap()
        .is_none());
    drop(prepared);
    assert!(load_capture(&checkpoints, 4, 1, &"c".repeat(40))
        .unwrap()
        .is_none());

    prepare_capture(&workspace, &checkpoints, 4, 2, &"c".repeat(40), 0)
        .unwrap()
        .commit()
        .unwrap();
    let retained = capture_path(&checkpoints, 4, 2);
    let mut corrupt = std::fs::read(&retained).unwrap();
    *corrupt.last_mut().unwrap() ^= 0xff;
    std::fs::write(retained, corrupt).unwrap();
    assert!(load_capture(&checkpoints, 4, 2, &"c".repeat(40)).is_err());
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn the_runtime_advertises_and_acknowledges_only_a_pre_start_verified_workspace() {
    let base = root("http");
    let target = base.join("workspace");
    let _ = std::fs::remove_dir_all(&base);
    let (archive, header) = fixture(vec![(
        "README.md",
        EntryKind::Regular,
        0o100644,
        b"hello\n",
    )]);
    let archive_sha = hex(&Sha256::digest(&archive));
    let baseline_digest = "f".repeat(64);
    let mut state = crate::AppState::for_test(&[]);
    state.workspace_root = target.clone();
    state.checkpoint_root = base.join("checkpoints");

    let readiness = crate::ready(State(state.clone())).await.into_response();
    let readiness = axum::body::to_bytes(readiness.into_body(), 64 * 1024)
        .await
        .expect("readiness body");
    let readiness: serde_json::Value = serde_json::from_slice(&readiness).unwrap();
    #[cfg(target_os = "linux")]
    assert_eq!(
        readiness["workspaceCheckpoint"]["version"],
        CAPABILITY_VERSION
    );
    #[cfg(not(target_os = "linux"))]
    assert!(readiness["workspaceCheckpoint"].is_null());
    assert_eq!(
        readiness["githubCapability"]["name"],
        "github_capability_v1"
    );
    assert_eq!(
        readiness["githubCapability"]["version"],
        crate::github_capability::POLICY_VERSION
    );
    assert_eq!(
        readiness["githubCapability"]["operationClasses"],
        serde_json::json!([
            "repository_read",
            "tree_read",
            "object_create",
            "launch_ref_create",
            "launch_ref_update",
            "status_read",
        ])
    );

    let params = crate::WorkspaceRestoreParams {
        epoch: 1,
        source: header.source_revision.clone(),
        baseline: baseline_digest.clone(),
        manifest: header.manifest_digest.clone(),
        archive: archive_sha.clone(),
    };
    let response = crate::workspace_restore(
        State(state.clone()),
        Query(params),
        axum::body::Bytes::from(archive.clone()),
    )
    .await
    .into_response();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let acknowledgement = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("acknowledgement body");
    let acknowledgement: serde_json::Value = serde_json::from_slice(&acknowledgement).unwrap();
    assert_eq!(acknowledgement["workspace_epoch"], 1);
    assert_eq!(acknowledgement["source_revision"], header.source_revision);
    assert_eq!(acknowledgement["baseline_manifest_digest"], baseline_digest);
    assert_eq!(acknowledgement["manifest_digest"], header.manifest_digest);
    assert_eq!(acknowledgement["entries"], 1);
    assert_eq!(acknowledgement["bytes"], 6);

    let wrong = crate::workspace_checkpoint(
        State(state.clone()),
        Query(crate::WorkspaceCheckpointParams {
            epoch: 1,
            version: 1,
            source: "b".repeat(40),
            baseline: baseline_digest.clone(),
        }),
    )
    .await
    .into_response();
    assert_eq!(wrong.status(), axum::http::StatusCode::CONFLICT);

    let wrong = crate::workspace_checkpoint(
        State(state.clone()),
        Query(crate::WorkspaceCheckpointParams {
            epoch: 1,
            version: 1,
            source: header.source_revision.clone(),
            baseline: header.manifest_digest.clone(),
        }),
    )
    .await
    .into_response();
    assert_eq!(wrong.status(), axum::http::StatusCode::CONFLICT);

    let checkpoint_params = crate::WorkspaceCheckpointParams {
        epoch: 1,
        version: 1,
        source: header.source_revision.clone(),
        baseline: baseline_digest.clone(),
    };
    #[cfg(not(target_os = "linux"))]
    {
        let response = crate::workspace_checkpoint(State(state), Query(checkpoint_params))
            .await
            .into_response();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        std::fs::remove_dir_all(base).unwrap();
    }
    #[cfg(target_os = "linux")]
    {
        let response = crate::workspace_checkpoint(
            State(state.clone()),
            Query(crate::WorkspaceCheckpointParams {
                epoch: checkpoint_params.epoch,
                version: checkpoint_params.version,
                source: checkpoint_params.source.clone(),
                baseline: checkpoint_params.baseline.clone(),
            }),
        )
        .await
        .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let first_checkpoint =
            axum::body::to_bytes(response.into_body(), MAX_ARCHIVE_BYTES + 64 * 1024)
                .await
                .expect("checkpoint wire");
        let prefix: [u8; 8] = first_checkpoint[..8].try_into().unwrap();
        let acknowledgement_end = 8 + usize::try_from(u64::from_be_bytes(prefix)).unwrap();
        let acknowledgement: serde_json::Value =
            serde_json::from_slice(&first_checkpoint[8..acknowledgement_end]).unwrap();
        assert_eq!(acknowledgement["checkpoint_version"], 1);
        assert_eq!(acknowledgement["writers_resumed"], true);

        std::fs::write(target.join("README.md"), b"changed after capture\n").unwrap();
        let response = crate::workspace_checkpoint(State(state.clone()), Query(checkpoint_params))
            .await
            .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let repeated_checkpoint =
            axum::body::to_bytes(response.into_body(), MAX_ARCHIVE_BYTES + 64 * 1024)
                .await
                .expect("repeated checkpoint wire");
        assert_eq!(repeated_checkpoint, first_checkpoint);

        let _started = state
            .registry
            .get_or_start("started", "/bin/echo", 80, 24, "placeholder")
            .await
            .expect("test agent starts");
        let response = crate::workspace_restore(
            State(state.clone()),
            Query(crate::WorkspaceRestoreParams {
                epoch: 1,
                source: header.source_revision,
                baseline: baseline_digest,
                manifest: header.manifest_digest,
                archive: archive_sha,
            }),
            axum::body::Bytes::from(archive),
        )
        .await
        .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
        state.registry.remove("started").await;
        std::fs::remove_dir_all(base).unwrap();
    }
}
