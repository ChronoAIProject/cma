use super::*;

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
