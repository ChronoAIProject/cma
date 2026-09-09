use std::path::Path;
use std::sync::Arc;

use layer3_adapter::wire::{FencedRequest, RequestFrame, ResponseFrame};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentHomeRemoveRequest, AgentLaunch, AgentSlot, HomeFile,
    InstallRequest, WorktreeRef, WorktreeRequest,
};

use super::{credential, failure, status, workspace, Fixture, Supervisor, SupervisorBoundary};

async fn installed(fixture: &Fixture) -> Arc<Supervisor> {
    let supervisor = fixture.supervisor();
    let response = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::new(
            "install",
            vec![],
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::Install(_)),
        "{response:?}"
    );
    let response = supervisor.dispatch(credential()).await;
    assert!(
        matches!(response, ResponseFrame::Credential(_)),
        "{response:?}"
    );
    supervisor
}

fn plan() -> AgentHomePlan {
    AgentHomePlan {
        operation_id: "agent:agent:home:create:v1".into(),
        request_sha256: "a".repeat(64),
        agent_id: AgentSlot::new("agent"),
        files: vec![HomeFile {
            path: "conversation-fixture".into(),
            bytes: b"original conversation contents".to_vec(),
        }],
        symlinks: vec![],
        skills: vec![],
    }
}

async fn prepare_agent(fixture: &Fixture, supervisor: &Supervisor) -> AgentLaunch {
    let response = supervisor.dispatch(workspace()).await;
    let ResponseFrame::PrepareWorkspace(workspace) = response else {
        panic!("{response:?}")
    };
    let request = WorktreeRequest::new(
        "worktree:project:agent:add:v1",
        "project",
        "agent",
        workspace.resolved_commit,
    );
    let response = supervisor
        .dispatch(RequestFrame::AddWorktree(FencedRequest::new(1, request)))
        .await;
    let ResponseFrame::AddWorktree(worktree) = response else {
        panic!("{response:?}")
    };
    let response = supervisor
        .dispatch(RequestFrame::MaterializeAgentHome(FencedRequest::new(
            1,
            plan(),
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::MaterializeAgentHome(_)),
        "{response:?}"
    );
    AgentLaunch {
        operation_id: "agent:agent:open:v1".into(),
        request_sha256: "b".repeat(64),
        agent_id: AgentSlot::new("agent"),
        cwd: worktree.receipt.path,
        home: fixture
            .config
            .agent_home("agent")
            .to_string_lossy()
            .into_owned(),
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "while IFS= read -r line; do :; done".into()],
        env: Default::default(),
    }
}

fn open(fence: u64, launch: &AgentLaunch) -> RequestFrame {
    RequestFrame::OpenAgent(FencedRequest::new(fence, launch.clone()))
}

fn close(fence: u64) -> RequestFrame {
    RequestFrame::CloseAgent(FencedRequest::new(
        fence,
        AgentCloseRequest::new("agent:agent:close:v1", AgentSlot::new("agent")),
    ))
}

async fn replace(fixture: &mut Fixture) -> Arc<Supervisor> {
    fixture.config.generation += 1;
    let replacement = fixture.supervisor();
    assert_eq!(
        status(replacement.dispatch(fixture.status()).await).agent_processes,
        0
    );
    let response = replacement.dispatch(credential()).await;
    assert!(
        matches!(response, ResponseFrame::Credential(_)),
        "{response:?}"
    );
    replacement
}

#[tokio::test]
async fn replacement_reopens_agent_process_while_preserving_filesystem_receipts() {
    let mut fixture = Fixture::new();
    let supervisor = installed(&fixture).await;
    let mut launch = prepare_agent(&fixture, &supervisor).await;
    let response = supervisor.dispatch(open(1, &launch)).await;
    assert!(
        matches!(response, ResponseFrame::OpenAgent(_)),
        "{response:?}"
    );
    assert_eq!(
        status(supervisor.dispatch(fixture.status()).await).agent_processes,
        1
    );
    let response = supervisor.dispatch(close(2)).await;
    assert!(
        matches!(response, ResponseFrame::CloseAgent(_)),
        "{response:?}"
    );
    // Cleanup still dominates even a newer OpenAgent request in the same runtime.
    assert_eq!(
        failure(supervisor.dispatch(open(3, &launch)).await),
        "mutation_authority_retired"
    );
    let home_file = fixture
        .config
        .agent_home("agent")
        .join("conversation-fixture");
    let metadata_before = std::fs::read(&home_file).unwrap();
    let previous = fixture.config.clone();
    drop(supervisor);
    let replacement = replace(&mut fixture).await;
    // Resuming a recorded conversation renders a different launch request digest.
    launch.request_sha256 = "c".repeat(64);
    let response = replacement.dispatch(open(3, &launch)).await;
    assert!(
        matches!(response, ResponseFrame::OpenAgent(_)),
        "{response:?}"
    );
    assert_eq!(std::fs::read(&home_file).unwrap(), metadata_before);
    assert_eq!(
        status(replacement.dispatch(fixture.status()).await).agent_processes,
        1
    );
    assert_eq!(
        failure(replacement.dispatch(close(2)).await),
        "mutation_authority_stale"
    );
    assert_eq!(
        status(replacement.dispatch(fixture.status()).await).agent_processes,
        1
    );
    assert!(Supervisor::new(previous.clone(), SupervisorBoundary::default()).is_err());
    // Filesystem receipt lookup remains global across process generations.
    let response = replacement
        .dispatch(RequestFrame::MaterializeAgentHome(FencedRequest::new(
            3,
            plan(),
        )))
        .await;
    assert!(
        matches!(response, ResponseFrame::MaterializeAgentHome(_)),
        "{response:?}"
    );
    let response = replacement.dispatch(close(4)).await;
    assert!(
        matches!(response, ResponseFrame::CloseAgent(_)),
        "{response:?}"
    );
}

#[tokio::test]
async fn abrupt_process_loss_requires_a_new_runtime_generation_before_reopen() {
    let mut fixture = Fixture::new();
    let supervisor = installed(&fixture).await;
    let launch = prepare_agent(&fixture, &supervisor).await;
    assert!(matches!(
        supervisor.dispatch(open(1, &launch)).await,
        ResponseFrame::OpenAgent(_)
    ));
    // Stop the actual child without completing its durable CloseAgent receipt.
    let bridge = supervisor.bridge("agent").await.unwrap();
    bridge.terminate().await.unwrap();
    drop(bridge);
    drop(supervisor);
    let restarted = fixture.supervisor();
    assert!(matches!(
        restarted.dispatch(credential()).await,
        ResponseFrame::Credential(_)
    ));
    assert_eq!(
        failure(restarted.dispatch(open(2, &launch)).await),
        "agent_process_not_live"
    );
    drop(restarted);
    let replacement = replace(&mut fixture).await;
    let response = replacement.dispatch(open(3, &launch)).await;
    assert!(
        matches!(response, ResponseFrame::OpenAgent(_)),
        "{response:?}"
    );
    assert!(matches!(
        replacement.dispatch(close(4)).await,
        ResponseFrame::CloseAgent(_)
    ));
}

#[tokio::test]
async fn new_process_generation_preserves_agent_home_and_worktree_deletion_tombstones() {
    let mut fixture = Fixture::new();
    let supervisor = installed(&fixture).await;
    let launch = prepare_agent(&fixture, &supervisor).await;
    let remove_home = RequestFrame::RemoveAgentHome(FencedRequest::new(
        2,
        AgentHomeRemoveRequest::new("agent:agent:home:remove:v1", AgentSlot::new("agent")),
    ));
    assert!(matches!(
        supervisor.dispatch(remove_home).await,
        ResponseFrame::RemoveAgentHome(_)
    ));
    let remove_tree = RequestFrame::RemoveWorktree(FencedRequest::new(
        2,
        WorktreeRef::new("worktree:project:agent:remove:v1", "project", "agent"),
    ));
    assert!(matches!(
        supervisor.dispatch(remove_tree).await,
        ResponseFrame::RemoveWorktree(_)
    ));
    drop(supervisor);
    let replacement = replace(&mut fixture).await;
    assert_eq!(
        failure(
            replacement
                .dispatch(RequestFrame::MaterializeAgentHome(FencedRequest::new(
                    3,
                    plan()
                )))
                .await
        ),
        "mutation_authority_retired"
    );
    let recreate = RequestFrame::AddWorktree(FencedRequest::new(
        3,
        WorktreeRequest::new("worktree:project:agent:add:v1", "project", "agent", "HEAD"),
    ));
    assert_eq!(
        failure(replacement.dispatch(recreate).await),
        "mutation_authority_retired"
    );
    assert!(!Path::new(&launch.home).exists());
    assert!(!Path::new(&launch.cwd).exists());
    assert_eq!(
        failure(replacement.dispatch(open(3, &launch)).await),
        "agent_home_invalid"
    );
}

#[tokio::test]
async fn lower_runtime_generation_cannot_mutate_after_home_authority_advances() {
    let mut fixture = Fixture::new();
    let supervisor = installed(&fixture).await;
    let previous = fixture.config.clone();
    let replacement = replace(&mut fixture).await;
    assert_eq!(
        failure(supervisor.dispatch(workspace()).await),
        "persistent_home_invalid"
    );
    assert_eq!(
        failure(supervisor.dispatch(credential()).await),
        "persistent_home_invalid"
    );
    assert!(Supervisor::new(previous, SupervisorBoundary::default()).is_err());
    let response = replacement.dispatch(workspace()).await;
    assert!(
        matches!(response, ResponseFrame::PrepareWorkspace(_)),
        "{response:?}"
    );
}

#[tokio::test]
async fn copied_old_process_records_are_rejected_in_the_new_generation() {
    for directory in ["runtime-mutations", "mutation-authority"] {
        let mut fixture = Fixture::new();
        let supervisor = installed(&fixture).await;
        let launch = prepare_agent(&fixture, &supervisor).await;
        assert!(matches!(
            supervisor.dispatch(open(1, &launch)).await,
            ResponseFrame::OpenAgent(_)
        ));
        assert!(matches!(
            supervisor.dispatch(close(2)).await,
            ResponseFrame::CloseAgent(_)
        ));
        let old = fixture
            .config
            .state_root
            .join("process-generation-1")
            .join(directory);
        drop(supervisor);
        let replacement = replace(&mut fixture).await;
        let new = fixture
            .config
            .state_root
            .join("process-generation-2")
            .join(directory);
        std::fs::create_dir_all(&new).unwrap();
        for entry in std::fs::read_dir(old).unwrap() {
            let entry = entry.unwrap();
            std::fs::copy(entry.path(), new.join(entry.file_name())).unwrap();
        }
        let refusal = failure(replacement.dispatch(open(3, &launch)).await);
        assert!(
            matches!(
                refusal.as_str(),
                "mutation_journal_corrupt" | "mutation_authority_corrupt"
            ),
            "{refusal}"
        );
        assert_eq!(
            status(replacement.dispatch(fixture.status()).await).agent_processes,
            0
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn process_generation_directories_must_not_alias_older_authority() {
    let mut fixture = Fixture::new();
    let supervisor = installed(&fixture).await;
    let launch = prepare_agent(&fixture, &supervisor).await;
    assert!(matches!(
        supervisor.dispatch(open(1, &launch)).await,
        ResponseFrame::OpenAgent(_)
    ));
    assert!(matches!(
        supervisor.dispatch(close(2)).await,
        ResponseFrame::CloseAgent(_)
    ));
    drop(supervisor);
    let replacement = replace(&mut fixture).await;
    let old = fixture.config.state_root.join("process-generation-1");
    let new = fixture.config.state_root.join("process-generation-2");
    std::os::unix::fs::symlink(&old, &new).unwrap();
    assert_eq!(
        failure(replacement.dispatch(open(3, &launch)).await),
        "mutation_scope_invalid"
    );
}
