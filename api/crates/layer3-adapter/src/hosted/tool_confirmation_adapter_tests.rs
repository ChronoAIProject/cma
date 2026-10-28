use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::hosted::{HostedAccessToolBinding, HostedAccessToolSource};

#[derive(Default)]
struct ConfirmingSource {
    resolves: AtomicUsize,
    confirmations: AtomicUsize,
    resolved: Mutex<Option<HostedAccessToolBinding>>,
    opened: Arc<Mutex<Option<HostedAccessToolBinding>>>,
    confirmed: Mutex<Option<String>>,
    reject_after_lost_commit: bool,
}

#[async_trait::async_trait]
impl HostedAccessToolSource for ConfirmingSource {
    async fn resolve(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        fence: u64,
    ) -> crate::Result<Option<HostedAccessToolBinding>> {
        self.resolves.fetch_add(1, Ordering::SeqCst);
        let binding = HostedAccessToolBinding::new(
            "https://cma.example",
            RuntimeCredential::new(format!("cma_at_{}", "d".repeat(64))),
            runtime.clone(),
            request,
            fence,
            None,
        )?;
        *self.resolved.lock().unwrap() = Some(binding.clone());
        Ok(Some(binding))
    }

    async fn confirm_open(
        &self,
        binding: &HostedAccessToolBinding,
        conversation: &str,
    ) -> crate::Result<()> {
        assert_eq!(self.resolved.lock().unwrap().as_ref(), Some(binding));
        assert_eq!(self.opened.lock().unwrap().as_ref(), Some(binding));
        if self.confirmations.fetch_add(1, Ordering::SeqCst) == 0 {
            *self.confirmed.lock().unwrap() = Some(conversation.into());
            return Err(AdapterError::Unavailable("commit response lost".into()));
        }
        assert_eq!(
            self.confirmed.lock().unwrap().as_deref(),
            Some(conversation)
        );
        if self.reject_after_lost_commit {
            Err(AdapterError::Rejected("owning lifecycle changed".into()))
        } else {
            Ok(())
        }
    }
}

#[tokio::test]
async fn completed_open_survives_ambiguous_confirmation_without_reinitialization() {
    for revoked in [false, true] {
        let backend = CorruptingBackend::default();
        let source = Arc::new(ConfirmingSource {
            reject_after_lost_commit: revoked,
            opened: backend.opened_access.clone(),
            ..Default::default()
        });
        let adapter = CodexAdapter::with_backend(backend.clone(), CodexSandboxSpec::default())
            .with_access_tools(source.clone());
        let (handle, workspace) = ready_runtime(&adapter, "sandbox-confirm", "confirm").await;
        let agent_id = "agent-confirm";
        let worktree = adapter
            .add_worktree(
                &handle,
                1,
                &WorktreeRequest::new(
                    format!("worktree:confirm:{agent_id}:add:v1"),
                    "confirm",
                    agent_id,
                    workspace.resolved_commit,
                ),
            )
            .await
            .unwrap();
        let options = runtime_options(&worktree, agent_id);
        adapter
            .materialize_agent_home(
                &handle,
                1,
                &AgentHomeRequest::new(
                    format!("agent:{agent_id}:home:create:v1"),
                    options.clone(),
                    "prompt",
                    Vec::new(),
                ),
            )
            .await
            .unwrap();
        let open = AgentOpenRequest::new(format!("agent:{agent_id}:open:v1"), options, None);
        let (updates, _receiver) = mpsc::channel(8);
        let result = adapter.open_agent(&handle, 1, &open, updates).await;
        if revoked {
            assert!(
                matches!(result, Err(AdapterError::Rejected(reason)) if reason == "owning lifecycle changed")
            );
        } else {
            let channel = result.unwrap();
            assert_eq!(channel.conversation_id, *source.confirmed.lock().unwrap());
        }
        assert_eq!(backend.opens.load(Ordering::SeqCst), 1);
        assert_eq!(source.resolves.load(Ordering::SeqCst), 1);
        assert_eq!(source.confirmations.load(Ordering::SeqCst), 2);
        assert_eq!(
            backend.inner.snapshot(&handle).await.unwrap().agents.len(),
            1
        );
    }
}
