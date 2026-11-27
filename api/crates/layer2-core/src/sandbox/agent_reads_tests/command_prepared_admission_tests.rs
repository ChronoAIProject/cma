use super::*;

pub(super) async fn complete_then_change_model(store: &InMemorySandboxStore) {
    use crate::sandbox::agent_operation_dispatch::{new_operation, operation_log, runtime_target};
    use crate::sandbox::store::{
        AgentConversationMutation, AgentConversationStore, SandboxMutationGuard,
    };

    let mut current = store
        .get_agent(&AgentId("agent".into()))
        .await
        .unwrap()
        .unwrap();
    let revision = current.revision;
    current.status = AgentStatus::Ready;
    current.ready_at = Some(NOW.into());
    current.work.step = AgentWorkStep::None;
    current.conversation.conversation_id = current.work.warm_conversation_id.clone();
    current.conversation.runtime_generation = Some(1);
    current.revision += 1;
    store.save_agent(revision, current.clone()).await.unwrap();

    // Settings require their real conversation transaction after startup is done;
    // the generic Agent save deliberately refuses changing settings.
    let parent = store
        .get_sandbox(&current.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let target = runtime_target(&current).unwrap();
    let sequence = current.conversation.last_seq + 1;
    let settings = AgentSettings {
        model: "changed-model".into(),
        ..current.settings()
    };
    let mut next = current.clone();
    next.conversation.settings = Some(settings.clone());
    next.conversation.operation = Some(
        new_operation(
            sequence,
            &target,
            sequence,
            AgentOperationIntent::UpdateSettings {
                settings,
                previous: current.conversation.settings.clone(),
            },
            NOW,
        )
        .unwrap(),
    );
    next.revision += 1;
    next.conversation.last_seq = sequence;
    let log = operation_log(&next, sequence).unwrap();
    store
        .commit_agent_conversation(AgentConversationMutation {
            settings_fence: None,
            command_claim: None,
            execution_fence: None,
            trigger_execution_fence: None,
            admission_guard: Some(SandboxMutationGuard::capture(&parent)),
            expected_agent_revision: current.revision,
            expected_last_seq: current.conversation.last_seq,
            runtime_generation: target.runtime_generation,
            agent: next,
            appended_at: NOW.into(),
            agent_logs: vec![log],
        })
        .await
        .unwrap();
}

// Native prepared adoption starts under an already Active parent, with the
// original profile Workspace and an existing conversation retained for adoption.
pub(super) async fn fixture() -> (Arc<InMemorySandboxStore>, Arc<Authority>, Agent) {
    let (store, authority, mut agent) = bootstrap_fixture().await;
    let mut parent = store.get_sandbox(&agent.sandbox_id).await.unwrap().unwrap();
    let revision = parent.revision;
    parent.status = SandboxStatus::Active;
    parent.work.step = SandboxWorkStep::WaitForCredentialRotation;
    parent.revision += 1;
    store.save_sandbox(revision, parent.clone()).await.unwrap();
    let mut workspace = store
        .get_workspace(&agent.workspace_id)
        .await
        .unwrap()
        .unwrap();
    let revision = workspace.revision;
    workspace.status = WorkspaceStatus::Ready;
    workspace.resolved_commit = Some("a".repeat(40));
    workspace.ready_at = Some(NOW.into());
    workspace.work.step = WorkspaceWorkStep::None;
    workspace.revision += 1;
    store
        .save_workspace(revision, workspace.clone())
        .await
        .unwrap();
    let revision = agent.revision;
    agent.work.step = AgentWorkStep::OpenConversation;
    agent.work.warm_conversation_id = Some("prepared-conversation".into());
    agent.revision += 1;
    store.save_agent(revision, agent.clone()).await.unwrap();
    assert_eq!(agent.bootstrap_lineage(&parent, &workspace), None);
    (store, authority, agent)
}

#[tokio::test]
async fn prepared_starting_revision_race_and_ready_publication_admit_once() {
    for progress in [Progress::Agent, Progress::PreparedReady] {
        let (store, authority, agent) = fixture().await;
        let before_log = store.agent_log(&agent.id, 0, 20).await.unwrap();
        let (service, authority) = service(&store, authority, progress);
        let accepted = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                submission("prepared-first-input"),
            )
            .await
            .unwrap();
        assert_eq!(authority.calls.load(Ordering::SeqCst), 2);
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
                    CommandSubmissionSource::Api,
                    submission("prepared-first-input"),
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
async fn prepared_refresh_preserves_explicit_revision_and_refuses_stop_or_policy_change() {
    for (progress, explicit) in [
        (Progress::Agent, true),
        (Progress::Stop, false),
        (Progress::Policy, false),
    ] {
        let (store, authority, agent) = fixture().await;
        let (service, authority) = service(&store, authority, progress);
        let mut request = submission("prepared-refused");
        request.expected_agent_revision = explicit.then_some(agent.revision);
        let error = service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
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
                CommandAdmissionError::Ledger(CommandLedgerError::KnownAborted)
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
async fn prepared_refresh_excludes_generic_agents_mismatched_lifecycle_and_controls() {
    for variation in 0..5 {
        let (store, authority, mut agent) = fixture().await;
        let revision = agent.revision;
        match variation {
            0 => agent.work.warm_conversation_id = None,
            1 => agent.origin = AgentOrigin::Manual,
            2 => {
                agent.origin = AgentOrigin::Lifecycle {
                    operation_id: "other-creation".into(),
                }
            }
            3 => agent.work.step = AgentWorkStep::None,
            4 => {}
            _ => unreachable!(),
        }
        agent.revision += 1;
        store.save_agent(revision, agent).await.unwrap();
        let (service, authority) = service(&store, authority, Progress::Agent);
        let mut request = submission("not-prepared-creation");
        if variation == 4 {
            request.command = AgentCommand::Interrupt;
        }
        assert!(matches!(
            service
                .admit(
                    &context("owner"),
                    "selected-credential",
                    CommandSubmissionSource::Api,
                    request,
                )
                .await,
            Err(CommandAdmissionError::Ledger(
                CommandLedgerError::KnownAborted
            ))
        ));
        assert_eq!(authority.calls.load(Ordering::SeqCst), 1);
        assert!(store
            .command_work_page(&PageRequest::default())
            .await
            .unwrap()
            .items
            .is_empty());
    }
}

#[tokio::test]
async fn prepared_refresh_refuses_a_model_changed_during_initial_authority_read() {
    let (store, authority, agent) = fixture().await;
    let (service, authority) = service(&store, authority, Progress::ModelChange);
    let mut request = submission("prepared-selected-model");
    request.requested_model = agent.selected_model();
    assert!(matches!(
        service
            .admit(
                &context("owner"),
                "selected-credential",
                CommandSubmissionSource::Api,
                request,
            )
            .await,
        Err(CommandAdmissionError::ModelMismatch)
    ));
    assert_eq!(authority.calls.load(Ordering::SeqCst), 2);
    assert!(store
        .command_work_page(&PageRequest::default())
        .await
        .unwrap()
        .items
        .is_empty());
}
