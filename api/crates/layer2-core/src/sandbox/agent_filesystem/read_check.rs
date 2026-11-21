use super::*;

impl AgentFileService {
    pub async fn check_read(
        &self,
        caller: &crate::Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        agent_id: &AgentId,
        path: &str,
    ) -> Result<(), AgentFileError> {
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
        runtime_request(
            &authorized.snapshot.lineage.sandbox,
            agent_id,
            path,
            false,
            |sandbox, _| {
                runtime(sandbox)?;
                generation(sandbox)?;
                Ok(())
            },
        )?;
        self.finish(&authorized, agent_id, ResourceCapability::ViewFiles)
            .await
    }
}
