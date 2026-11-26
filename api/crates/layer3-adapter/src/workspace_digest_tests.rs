use super::*;

#[test]
fn preparation_and_setup_digests_bind_configured_limits() {
    let prepare = WorkspaceRequest::with_clone_depth(
        "prepare",
        "workspace",
        "repo",
        WorkspaceSource::Init,
        2,
    );
    let mut changed = prepare.clone();
    changed.clone_depth = 3;
    assert!(!changed.has_valid_sha256());
    let setup = WorkspaceSetupRequest::with_timeouts(
        "setup",
        "workspace",
        "repo",
        Vec::new(),
        20,
        30,
        1_024,
    );
    let mut changed = setup.clone();
    changed.total_timeout_seconds = 31;
    assert!(!changed.has_valid_sha256());
    assert_eq!(
        serde_json::from_value::<WorkspaceRequest>(serde_json::to_value(&prepare).unwrap())
            .unwrap(),
        prepare
    );
    assert_eq!(
        serde_json::from_value::<WorkspaceSetupRequest>(serde_json::to_value(&setup).unwrap())
            .unwrap(),
        setup
    );
}

#[test]
fn worktree_digest_encodings_are_stable() {
    let add = WorktreeRequest::new(
        "worktree:repo:agent-a:add:v1",
        "repo",
        "agent-a",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    );
    let remove = WorktreeRef::new("worktree:repo:agent-a:remove:v1", "repo", "agent-a");
    assert_eq!(
        [add.request_sha256.as_str(), remove.request_sha256.as_str()],
        [
            "117178debf0682a9fec50ca2c56644fa61d96e36ca79e60c49dc65d11a95bc2c",
            "fb82cad8a2d55198865c44caf4ecc9aebac4a86ea1070e13c564347f688189e7",
        ]
    );
}

#[test]
fn worktree_digests_cover_every_execution_field() {
    let add = WorktreeRequest::new(
        "worktree:repo:agent-a:add:v1",
        "repo",
        "agent-a",
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    );
    let mut add_digests = Vec::new();
    for change in 0..3 {
        let mut changed = add.clone();
        match change {
            0 => changed.workspace.push_str("-other"),
            1 => changed.name.push_str("-other"),
            _ => changed.base.replace_range(..1, "f"),
        }
        add_digests.push(changed.computed_sha256());
    }
    assert!(add_digests
        .iter()
        .all(|digest| digest != &add.request_sha256));

    let remove = WorktreeRef::new("worktree:repo:agent-a:remove:v1", "repo", "agent-a");
    let mut changed_workspace = remove.clone();
    changed_workspace.workspace.push_str("-other");
    let mut changed_name = remove.clone();
    changed_name.name.push_str("-other");
    assert_ne!(changed_workspace.computed_sha256(), remove.request_sha256);
    assert_ne!(changed_name.computed_sha256(), remove.request_sha256);
}

#[test]
fn public_repository_archive_binds_the_canonical_locator_and_commit() {
    let commit = "a".repeat(40);
    let repository = "https://github.com/example/project";
    assert_eq!(
        public_repository_archive_url(repository, &commit)
            .unwrap()
            .as_str(),
        format!("https://codeload.github.com/example/project/zip/{commit}")
    );
    for invalid in [
        "https://github.com/example",
        "https://github.com/example/project/",
        "https://example.com/example/project",
    ] {
        assert!(public_repository_archive_url(invalid, &commit).is_none());
    }
    assert!(public_repository_archive_url(repository, "main").is_none());
    let public = WorkspaceRequest::new(
        "prepare",
        "workspace",
        "repo",
        WorkspaceSource::PublicRepositorySnapshot {
            url: repository.into(),
            commit: commit.clone(),
        },
    );
    let ordinary = WorkspaceRequest::new(
        "prepare",
        "workspace",
        "repo",
        WorkspaceSource::RepositorySnapshot {
            url: repository.into(),
            reference: Some(commit),
        },
    );
    assert_ne!(public.request_sha256, ordinary.request_sha256);
    assert_eq!(
        serde_json::from_value::<WorkspaceRequest>(serde_json::to_value(&public).unwrap()).unwrap(),
        public
    );
}
