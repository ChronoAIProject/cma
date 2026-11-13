use super::*;

impl AgentFileService {
    pub async fn read(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
        max_bytes: u64,
    ) -> Result<AgentFileReadReceipt, AgentFileError> {
        let authorized = self
            .authorize(
                caller,
                context,
                credential,
                agent_id,
                ResourceCapability::ViewFiles,
                AccessPurpose::Read,
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
        self.finish(&authorized, agent_id, ResourceCapability::ViewFiles)
            .await?;
        Ok(response)
    }
}
