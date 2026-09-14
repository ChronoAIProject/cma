use super::*;

const SANDBOX: &str = "sandbox-restored-open-replay";
const CONVERSATION: &str = "recorded-restored-conversation";

async fn fixture(
    legacy: bool,
) -> (
    CorruptingBackend,
    CodexAdapter,
    RuntimeHandle,
    AgentOpenRequest,
) {
    let backend = CorruptingBackend::default();
    let adapter = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
    let (handle, workspace) = ready_runtime(&adapter, SANDBOX, "restored").await;
    let worktree = adapter
        .add_worktree(
            &handle,
            1,
            &WorktreeRequest::new(
                "worktree:restored:wt-1:add:v1",
                "restored",
                "wt-1",
                workspace.resolved_commit,
            ),
        )
        .await
        .unwrap();
    let options = runtime_options(&worktree, "agent-restored");
    adapter
        .materialize_agent_home(
            &handle,
            1,
            &AgentHomeRequest::new(
                "agent:agent-restored:home:create:v1",
                options.clone(),
                "original system prompt",
                Vec::new(),
            ),
        )
        .await
        .unwrap();
    let original = AgentOpenRequest::new("agent:agent-restored:open:v1", options, None);
    let (updates, _receiver) = mpsc::channel(4);
    if legacy {
        let restored = AgentOpenRequest::new(
            original.operation_id.clone(),
            original.options.clone(),
            Some(CONVERSATION.into()),
        );
        adapter
            .open_agent(&handle, 2, &restored, updates)
            .await
            .unwrap();
    } else {
        adapter
            .reconnect_agent(&handle, 2, &original, CONVERSATION, updates)
            .await
            .unwrap();
    }
    (backend, adapter, handle, original)
}

async fn open_requests(backend: &CorruptingBackend) -> Vec<FencedRequest<AgentLaunch>> {
    backend
        .inner
        .requests()
        .await
        .into_iter()
        .filter_map(|(_, request)| match request {
            RequestFrame::OpenAgent(launch) => Some(launch),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn restored_journal_reconnects_after_adapter_restart_and_checkpoint_churn() {
    for legacy in [false, true] {
        let (backend, adapter, handle, original) = fixture(legacy).await;
        let recorded = backend.inner.snapshot(&handle).await.unwrap();
        if legacy {
            let (updates, _receiver) = mpsc::channel(4);
            assert!(matches!(
                adapter.open_agent(&handle, 3, &original, updates).await,
                Err(AdapterError::OperationIdentityConflict(_))
            ));
        }
        drop(adapter);
        for epoch in 1..=3 {
            let restarted =
                CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default());
            assert_eq!(
                restarted
                    .provision(&SandboxSpec {
                        sandbox_id: SANDBOX.into(),
                        ..SandboxSpec::default()
                    })
                    .await
                    .unwrap(),
                handle
            );
            restarted
                .checkpoint(
                    &handle,
                    &CheckpointRequest {
                        epoch,
                        exclusions: Vec::new(),
                    },
                )
                .await
                .unwrap();
            let before = open_requests(&backend).await.len();
            let (updates, _receiver) = mpsc::channel(4);
            let channel = restarted
                .reconnect_agent(&handle, epoch + 3, &original, CONVERSATION, updates)
                .await
                .unwrap();
            assert_eq!(channel.conversation_id.as_deref(), Some(CONVERSATION));
            let requests = open_requests(&backend).await;
            let attempts = &requests[before..];
            assert_eq!(attempts.len(), if legacy { 2 } else { 1 });
            assert_eq!(attempts[0].request.request_sha256, original.request_sha256);
            assert_eq!(
                attempts.last().unwrap().request,
                recorded.agents[&original.options.slot]
            );
            assert!(attempts
                .iter()
                .all(|request| request.authority_fence == epoch + 3));
            let after = backend.inner.snapshot(&handle).await.unwrap();
            assert_eq!(after.agents, recorded.agents);
            assert_eq!(after.homes, recorded.homes);
            assert_eq!(after.worktrees, recorded.worktrees);
            assert!(original.resume_conversation.is_none());
        }
    }
}

#[tokio::test]
async fn reconnect_never_reinterprets_general_rejection_or_ambiguous_response() {
    let (backend, adapter, handle, original) = fixture(false).await;
    for ambiguous in [false, true] {
        if ambiguous {
            backend.inner.lose_next_response(WireOperation::OpenAgent);
        } else {
            backend.inner.fail_next(
                WireOperation::OpenAgent,
                "permission_denied",
                "mutation_authority_conflict",
            );
        }
        let before = open_requests(&backend).await.len();
        let (updates, _receiver) = mpsc::channel(4);
        let result = adapter
            .reconnect_agent(&handle, 3, &original, CONVERSATION, updates)
            .await;
        if ambiguous {
            unavailable(result);
        } else {
            rejected(result);
        }
        assert_eq!(open_requests(&backend).await.len(), before + 1);
    }
}

#[tokio::test]
async fn alternate_reconnect_requires_exact_options_conversation_and_receipt() {
    for corruption in ["options", "conversation", "receipt"] {
        let (backend, adapter, handle, mut original) = fixture(true).await;
        let recorded = backend.inner.snapshot(&handle).await.unwrap().agents;
        let conversation = if corruption == "conversation" {
            "different-recorded-conversation"
        } else {
            CONVERSATION
        };
        if corruption == "options" {
            original.options.model = Some("changed-model".into());
            original.request_sha256 = original.computed_sha256();
        }
        if corruption == "receipt" {
            backend.corrupt_next(Corruption::AgentOpenIdentity);
        }
        let before = open_requests(&backend).await.len();
        let (updates, _receiver) = mpsc::channel(4);
        assert!(adapter
            .reconnect_agent(&handle, 3, &original, conversation, updates)
            .await
            .is_err());
        assert_eq!(open_requests(&backend).await.len(), before + 2);
        assert_eq!(
            backend.inner.snapshot(&handle).await.unwrap().agents,
            recorded
        );
    }
}
