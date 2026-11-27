use super::*;
use crate::sandbox::store::{SandboxAdmission, SandboxAdmissionName, SandboxAdmissionStore};

#[path = "command_bootstrap_retry_results_tests.rs"]
mod retry_results;

#[path = "queued_edit_retry_tests.rs"]
mod queued_edits;

#[path = "command_prepared_admission_tests.rs"]
mod prepared;

#[derive(Clone, Copy)]
enum Progress {
    Parent,
    Agent,
    ConversationOutput,
    ConversationUsage,
    TwoAgentRevisions,
    PreparedReady,
    ModelChange,
    Stop,
    Policy,
    EveryAttempt,
    SlowRefresh,
    SlowFirstPreparation,
    ChangedScope,
}

struct ProgressingAuthority {
    inner: Arc<Authority>,
    store: Arc<InMemorySandboxStore>,
    progress: Progress,
    calls: AtomicUsize,
}

#[async_trait]
impl AgentCommandAdmissionAuthority for ProgressingAuthority {
    async fn command_admission_authority(
        &self,
        context: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 && matches!(self.progress, Progress::SlowFirstPreparation) {
            tokio::time::sleep(Duration::from_secs(14)).await;
        }
        if call > 0 && matches!(self.progress, Progress::SlowRefresh) {
            tokio::time::sleep(Duration::from_secs(16)).await;
        }
        if call > 0 && matches!(self.progress, Progress::ChangedScope) {
            return Ok(AgentAuthorizationRevision {
                replay_principal: None,
                authority_id: "replacement-custody".into(),
                policy_revision: 2,
                grant_revision: None,
            });
        }
        if call == 0
            || matches!(self.progress, Progress::EveryAttempt)
            || (call == 1 && matches!(self.progress, Progress::TwoAgentRevisions))
        {
            match self.progress {
                Progress::Agent
                | Progress::ConversationOutput
                | Progress::TwoAgentRevisions
                | Progress::PreparedReady
                | Progress::Stop => {
                    let mut agent = self
                        .store
                        .get_agent(&AgentId("agent".into()))
                        .await
                        .unwrap()
                        .unwrap();
                    let revision = agent.revision;
                    agent.revision += 1;
                    if matches!(self.progress, Progress::ConversationOutput) {
                        let appended = self
                            .store
                            .append_agent_log(
                                &agent.id,
                                NOW.into(),
                                AgentLogKind::AgentMessage {
                                    content: "Output from the active turn".into(),
                                },
                            )
                            .await
                            .unwrap();
                        agent.conversation.last_seq = appended.seq;
                    }
                    if matches!(self.progress, Progress::Stop) {
                        agent.requested_stop_reason = Some(AgentStopReason::Owner);
                    }
                    if matches!(self.progress, Progress::PreparedReady) {
                        agent.status = AgentStatus::Ready;
                        agent.ready_at = Some(NOW.into());
                        agent.work.step = AgentWorkStep::PublishConnection;
                        agent.conversation.conversation_id =
                            agent.work.warm_conversation_id.clone();
                        agent.conversation.runtime_generation = Some(1);
                    }
                    self.store.save_agent(revision, agent).await.unwrap();
                }
                Progress::ConversationUsage => {
                    let agent = self
                        .store
                        .get_agent(&AgentId("agent".into()))
                        .await
                        .unwrap()
                        .unwrap();
                    let result = crate::sandbox::conversation_observation_fold::usage(
                        &agent,
                        &crate::sandbox::AgentMessageAssembly::default(),
                        &layer3_adapter::ConversationUsage {
                            turn_id: "active-provider-turn".into(),
                            input_tokens: 200,
                            output_tokens: 30,
                            last_input_tokens: 200,
                            last_output_tokens: 30,
                            context_used: 230,
                            context_window: Some(1000),
                            counters_reset: false,
                        },
                        NOW,
                    )
                    .unwrap();
                    let crate::sandbox::conversation_fold::FoldedConversationUpdate::Changed(
                        change,
                    ) = result
                    else {
                        panic!("active usage must update the Agent");
                    };
                    for log in change.logs {
                        self.store
                            .append_agent_log(&agent.id, NOW.into(), log)
                            .await
                            .unwrap();
                    }
                    self.store
                        .save_agent(agent.revision, change.agent)
                        .await
                        .unwrap();
                }
                Progress::SlowFirstPreparation => {}
                Progress::ModelChange => prepared::complete_then_change_model(&self.store).await,
                Progress::Policy => {
                    share(&self.store, None).await;
                }
                Progress::Parent
                | Progress::EveryAttempt
                | Progress::SlowRefresh
                | Progress::ChangedScope => {
                    let mut parent = self
                        .store
                        .get_sandbox(&SandboxId("sandbox".into()))
                        .await
                        .unwrap()
                        .unwrap();
                    let revision = parent.revision;
                    parent.revision += 1;
                    parent.work.step = SandboxWorkStep::MintInitialCredential;
                    self.store.save_sandbox(revision, parent).await.unwrap();
                }
            }
        }
        self.inner
            .command_admission_authority(context, credential, owner)
            .await
    }
}

#[tokio::test]
async fn queued_message_refreshes_after_output_from_the_same_active_turn() {
    assert_queued_message_refreshes(Progress::ConversationOutput).await;
}

#[tokio::test]
async fn queued_message_refreshes_after_usage_from_the_same_active_turn() {
    assert_queued_message_refreshes(Progress::ConversationUsage).await;
}

async fn assert_queued_message_refreshes(progress: Progress) {
    let (store, authority, mut agent) = retry_results::ordinary_fixture().await;
    let revision = agent.revision;
    agent.status = AgentStatus::Working;
    agent.conversation.active_turn = Some(1);
    agent.revision += 1;
    store.save_agent(revision, agent).await.unwrap();

    let (service, progressing) = service(&store, authority, progress);
    let owner = context("owner");
    let request = submission("queued-during-output");
    let accepted = service
        .admit(
            &owner,
            "selected-credential",
            CommandSubmissionSource::Api,
            request.clone(),
        )
        .await
        .unwrap();
    assert_eq!(accepted.status, AgentResponseStatus::Queued);
    assert_eq!(progressing.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        service
            .lookup_admitted(
                &owner,
                "selected-credential",
                CommandSubmissionSource::Api,
                request
            )
            .await
            .unwrap(),
        Some(accepted)
    );
    assert_eq!(
        store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
}

async fn bootstrap_fixture() -> (Arc<InMemorySandboxStore>, Arc<Authority>, Agent) {
    let store = Arc::new(InMemorySandboxStore::default());
    let authority = Arc::new(Authority::new());
    let mut agent = agent(
        "agent",
        "worktree",
        "workspace",
        "sandbox",
        "owner",
        AgentStatus::Starting,
    );
    let mut workspace = workspace("workspace", "sandbox", "owner");
    workspace.status = WorkspaceStatus::Preparing;
    workspace.origin = WorkspaceOrigin::SandboxProfile { index: 0 };
    workspace.resolved_commit = None;
    workspace.ready_at = None;
    workspace.work = WorkspaceWorkState::new(WorkspaceWorkStep::Prepare, NOW.into());
    let mut parent = sandbox("sandbox", "owner");
    parent.status = SandboxStatus::Provisioning;
    parent.origin = SandboxOrigin::Lifecycle {
        operation_id: "bootstrap-race".into(),
    };
    parent.work = SandboxWorkState::new(SandboxWorkStep::ProvisionRuntime, NOW.into());
    parent.profile.content.workspace_profiles = vec![crate::profiles::WorkspaceProfileRef::new(
        workspace.profile.id.clone(),
        workspace.profile.revision,
    )];
    parent.profile.digest = crate::profiles::content_digest(&parent.profile.content).unwrap();
    store
        .admit_sandbox(
            SandboxAdmission::new(
                parent.clone(),
                SandboxAdmissionName::Exact(parent.name.clone()),
                8,
                NOW.into(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    parent.revision += 1;
    parent.status = SandboxStatus::Installing;
    parent.work.step = SandboxWorkStep::RunInstall;
    store.save_sandbox(1, parent.clone()).await.unwrap();
    store.create_workspace(workspace.clone()).await.unwrap();
    agent.origin = AgentOrigin::Lifecycle {
        operation_id: "bootstrap-race".into(),
    };
    agent.work = AgentWorkState::new(AgentWorkStep::WaitForWorkspace, NOW.into());
    let mut worktree = worktree("worktree", "workspace", "sandbox", "agent", "owner");
    worktree.status = WorktreeStatus::Creating;
    worktree.base_commit.clear();
    insert_agent(&store, agent.clone(), worktree).await;
    assert_eq!(
        agent.bootstrap_lineage(&parent, &workspace),
        Some("bootstrap-race")
    );
    (store, authority, agent)
}

fn service(
    store: &Arc<InMemorySandboxStore>,
    authority: Arc<Authority>,
    progress: Progress,
) -> (Arc<AgentCommandService>, Arc<ProgressingAuthority>) {
    let access = Arc::new(ResourceAccessService::new(store.clone(), authority.clone()));
    let authority = Arc::new(ProgressingAuthority {
        inner: authority,
        store: store.clone(),
        progress,
        calls: AtomicUsize::new(0),
    });
    (
        AgentCommandService::new(store.clone(), access, authority.clone()),
        authority,
    )
}

#[tokio::test]
async fn bootstrap_revision_progress_during_authority_io_admits_once_without_dispatch() {
    for progress in [Progress::Parent, Progress::Agent] {
        let (store, authority, agent) = bootstrap_fixture().await;
        let before_log = store.agent_log(&agent.id, 0, 20).await.unwrap();
        let (service, authority) = service(&store, authority, progress);
        let accepted = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::UserInterface,
                submission("early-input"),
            )
            .await
            .unwrap();
        assert_eq!(authority.calls.load(Ordering::SeqCst), 2);
        assert_eq!(accepted.status, AgentResponseStatus::Queued);
        let record = store
            .get_command(&accepted.receipt.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.receipt.agent_order, 1);
        assert!(record.work.dispatch.is_none());
        assert_eq!(store.agent_log(&agent.id, 0, 20).await.unwrap(), before_log);
        assert_eq!(
            service
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::UserInterface,
                    submission("early-input"),
                )
                .await
                .unwrap(),
            accepted
        );
        assert_eq!(
            store
                .command_work_page(&PageRequest::default())
                .await
                .unwrap()
                .items
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn bootstrap_refresh_preserves_explicit_revision_and_refuses_stop_or_policy_change() {
    for (progress, explicit) in [
        (Progress::Parent, true),
        (Progress::Stop, false),
        (Progress::Policy, false),
    ] {
        let (store, authority, agent) = bootstrap_fixture().await;
        let (service, authority) = service(&store, authority, progress);
        let mut request = submission("closed-input");
        request.expected_agent_revision = explicit.then_some(agent.revision);
        let error = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::UserInterface,
                request,
            )
            .await
            .unwrap_err();
        match progress {
            Progress::Stop => assert!(matches!(
                error,
                CommandAdmissionError::Ledger(CommandLedgerError::NotAdmissible)
            )),
            Progress::Policy => assert!(matches!(
                error,
                CommandAdmissionError::Access(ResourceAccessError::Store(
                    ResourceAccessStoreError::Conflict
                ))
            )),
            _ => assert!(matches!(
                error,
                CommandAdmissionError::Ledger(CommandLedgerError::Conflict)
            )),
        }
        assert_eq!(
            authority.calls.load(Ordering::SeqCst),
            if matches!(progress, Progress::Stop) {
                2
            } else {
                1
            }
        );
        assert!(store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

#[tokio::test]
async fn bootstrap_refresh_stops_after_bounded_conflicts_without_a_receipt() {
    let (store, authority, _) = bootstrap_fixture().await;
    let (service, authority) = service(&store, authority, Progress::EveryAttempt);
    assert!(matches!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::UserInterface,
                submission("contended-input"),
            )
            .await,
        Err(CommandAdmissionError::Ledger(CommandLedgerError::Conflict))
    ));
    assert_eq!(authority.calls.load(Ordering::SeqCst), 3);
    assert!(store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
