use super::*;

#[tokio::test]
async fn successor_worktree_keeps_indeterminate_journal_partial_files_and_branch_after_restart() {
    let config = RuntimeConfig::for_test(root("retained-worktree-successor"));
    let supervisor = provision(&config).await;
    let workspace = prepare_workspace(&supervisor).await;
    let original = WorktreeRequest::new(
        "worktree:project:wt-1:add:v1",
        "project",
        "wt-1",
        workspace.resolved_commit.clone(),
    );
    let operation = Operation::add_worktree(&original);
    crate::mutation_authority::admit(&config, 1, &operation).unwrap();
    let ticket = match mutation_journal::begin::<WorktreeReceipt>(&config, &operation).unwrap() {
        Begin::New(ticket) => ticket,
        _ => panic!("fixture requires a new exact mutation"),
    };
    let partial = config.home.join("project/.worktrees/wt-1");
    std::fs::create_dir_all(&partial).unwrap();
    std::fs::write(
        partial.join("retained-user-data"),
        b"Never overwrite these files",
    )
    .unwrap();
    let git = crate::git::GitRunner::new(true);
    git.checked(
        &config.home.join("project"),
        ["branch", "cma/wt-1", &workspace.resolved_commit],
        "create retained branch",
    )
    .await
    .unwrap();
    let error = ticket
        .indeterminate(
            "worktree addition left partial or mismatched Git state; explicit recovery is required",
        )
        .unwrap();
    assert_eq!(error.code, "runtime_mutation_outcome_unknown");
    let journal = config.state_root.join("runtime-mutations").join(format!(
        "{:x}.json",
        Sha256::digest(original.operation_id.as_bytes())
    ));
    let bytes = std::fs::read(&journal).unwrap();
    let successor = WorktreeRequest::new(
        "worktree:project:wt-2:add:v1",
        "project",
        "wt-2",
        workspace.resolved_commit.clone(),
    );
    let response = supervisor
        .dispatch(RequestFrame::AddWorktree(FencedRequest::new(
            2,
            successor.clone(),
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::AddWorktree(_)),
        "{response:?}"
    );
    assert_eq!(std::fs::read(&journal).unwrap(), bytes);
    drop(supervisor);
    let supervisor = recreate(&config).await;
    assert_eq!(
        supervisor
            .dispatch(RequestFrame::AddWorktree(FencedRequest::new(2, successor)))
            .await,
        response
    );
    assert_eq!(
        failure(supervisor.dispatch(add_worktree_request(original)).await).code,
        "runtime_mutation_outcome_unknown"
    );
    assert_eq!(std::fs::read(&journal).unwrap(), bytes);
    assert_eq!(
        std::fs::read(partial.join("retained-user-data")).unwrap(),
        b"Never overwrite these files"
    );
    let branch = git
        .checked(
            &config.home.join("project"),
            ["rev-parse", "refs/heads/cma/wt-1"],
            "inspect retained branch",
        )
        .await
        .unwrap();
    assert_eq!(branch.stdout.trim(), workspace.resolved_commit);
    let successor_head = git
        .checked(
            &config.home.join("project/.worktrees/wt-2"),
            ["rev-parse", "HEAD"],
            "inspect successor",
        )
        .await
        .unwrap();
    assert_eq!(successor_head.stdout.trim(), workspace.resolved_commit);
    cleanup(&config);
}
