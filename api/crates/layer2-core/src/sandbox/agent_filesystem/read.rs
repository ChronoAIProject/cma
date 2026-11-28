use super::*;
use crate::diagnostic_context::DiagnosticContext;
use layer3_adapter::file_read_diagnostics::{self, measure, Stage};

impl AgentFileService {
    pub fn with_read_timings(self: Arc<Self>, enabled: bool) -> Arc<Self> {
        Arc::new(Self {
            access: self.access.clone(),
            adapter: self.adapter.clone(),
            task_experience: self.task_experience.clone(),
            read_timings: enabled,
            environment_cache: self.environment_cache.clone(),
        })
    }

    pub async fn read(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
        max_bytes: u64,
    ) -> Result<AgentFileReadReceipt, AgentFileError> {
        let operation = self.read_inner(caller, context, credential, agent_id, path, max_bytes);
        if !self.read_timings {
            return operation.await;
        }
        let diagnostic = DiagnosticContext::current();
        file_read_diagnostics::scope(
            move |stage, outcome, elapsed_us| {
                // JSON formatters include the current scope even for a
                // parentless event. Isolate it at the same level and target so
                // restrictive filters cannot leave a private parent attached.
                let span = tracing::info_span!(target: "agent_file_read_phase", parent: None, "agent_file_read");
                if span.is_disabled() {
                    return;
                }
                let _entered = span.enter();
                tracing::info!(target: "agent_file_read_phase", parent: &span,
                phase = stage.label(), outcome = outcome.label(), elapsed_us,
                route = diagnostic.route.as_ref(), request_hash = %diagnostic.request_hash,
                "agent file read phase");
            },
            operation,
        )
        .await
    }

    async fn read_inner(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
        max_bytes: u64,
    ) -> Result<AgentFileReadReceipt, AgentFileError> {
        let authorized = measure(
            Stage::Authorization,
            self.authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ViewFiles,
                AccessPurpose::Read,
            ),
        )
        .await?;
        let request = runtime_request(
            &authorized.snapshot.lineage.sandbox,
            agent_id,
            path,
            false,
            |sandbox, boot_id| {
                Ok(AgentFileReadRequest {
                    owner_access: authorized
                        .decision
                        .allows(ResourceCapability::ManageSharing),
                    protected_paths: Vec::new(),
                    sandbox_id: sandbox.id.to_string(),
                    runtime_generation: generation(sandbox)?,
                    boot_id,
                    home_identity: None,
                    path: path.into(),
                    max_bytes,
                })
            },
        )?;
        let response = self
            .adapter
            .read_agent_file(
                &RuntimeHandle(runtime(&authorized.snapshot.lineage.sandbox)?),
                &request,
            )
            .await?;
        measure(
            Stage::FinalFence,
            self.finish(&authorized, agent_id, ResourceCapability::ViewFiles),
        )
        .await?;
        Ok(response)
    }
}
