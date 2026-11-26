//! Operation execution after the supervisor's authorization and mutation gates.
use super::*;
use crate::paths::safe_component;

impl Supervisor {
    pub(super) async fn execute(&self, request: RequestFrame) -> Result<ResponseFrame> {
        if !matches!(
            request,
            RequestFrame::Install(_)
                | RequestFrame::BootstrapRestore(_)
                | RequestFrame::Credential(_)
                | RequestFrame::CredentialForReceiver { .. }
                | RequestFrame::Restore(_)
                | RequestFrame::ResumeCheckpointWriters(_)
                | RequestFrame::Resources
        ) {
            self.authorize_preparation(&request).await?;
        }
        let filesystem_identity = match &request {
            RequestFrame::ListAgentFiles(r) => {
                Some((&r.sandbox_id, r.runtime_generation, &r.boot_id))
            }
            RequestFrame::ReadAgentFile(r) => {
                Some((&r.sandbox_id, r.runtime_generation, &r.boot_id))
            }
            RequestFrame::WriteAgentFile(r) => {
                Some((&r.sandbox_id, r.runtime_generation, &r.boot_id))
            }
            RequestFrame::DeleteAgentFile(r) => {
                Some((&r.sandbox_id, r.runtime_generation, &r.boot_id))
            }
            _ => None,
        };
        if let Some((sandbox, generation, boot)) = filesystem_identity {
            if sandbox != &self.config.sandbox_id
                || generation != self.config.generation
                || boot != &self.boot_id
            {
                return Err(RuntimeError::rejected(
                    "filesystem_identity",
                    "filesystem request names a different runtime process",
                ));
            }
        }
        match request {
            RequestFrame::Browser(_) => unreachable!("browser observation dispatches separately"),
            RequestFrame::BrowserControl(_) => {
                unreachable!("browser control dispatches separately")
            }
            RequestFrame::UploadAgentAttachment(upload) => self
                .upload_attachment(upload)
                .await
                .map(ResponseFrame::UploadAgentAttachment),
            RequestFrame::ListAgentFiles(request) => Ok(ResponseFrame::ListAgentFiles(
                crate::agent_filesystem::list(&self.config, request)?,
            )),
            RequestFrame::ReadAgentFile(request) => Ok(ResponseFrame::ReadAgentFile(
                crate::agent_filesystem::read(&self.config, request)?,
            )),
            RequestFrame::WriteAgentFile(request) => Ok(ResponseFrame::WriteAgentFile(
                crate::agent_filesystem::write(&self.config, request)?,
            )),
            RequestFrame::DeleteAgentFile(request) => Ok(ResponseFrame::DeleteAgentFile(
                crate::agent_filesystem::delete(&self.config, request)?,
            )),
            RequestFrame::CacheSkillArchive(archive) => {
                crate::skills::cache_archive(&self.config, &archive)?;
                Ok(ResponseFrame::CacheSkillArchive {
                    sha256: archive.sha256,
                })
            }
            RequestFrame::Install(request) => self.install(request).await,
            RequestFrame::BootstrapRestore(request) => self.bootstrap_restore(request).await,
            RequestFrame::Credential(frame) if frame.github_route.is_none() => {
                self.credential(frame).await
            }
            RequestFrame::Credential(_) => Err(RuntimeError::rejected(
                "credential_receiver_required",
                "a bound credential requires the negotiated runtime receiver",
            )),
            RequestFrame::CredentialForReceiver {
                receiver,
                boot_id,
                credential,
            } => {
                if credential.github_route.is_none()
                    || receiver.sandbox_id != self.config.sandbox_id
                    || receiver.generation != self.config.generation
                    || Some(&receiver.home_identity) != self.config.home_identity.as_ref()
                    || boot_id != self.boot_id
                {
                    return Err(RuntimeError::rejected(
                        "credential_receiver_changed",
                        "credential receiver does not match the negotiated runtime",
                    ));
                }
                self.runtime_status(receiver).await?;
                self.credential(credential).await
            }
            RequestFrame::RecoverWorkspacePrepare(_)
            | RequestFrame::PrepareWorkspace(_)
            | RequestFrame::WorkspaceSetup(_)
            | RequestFrame::RemoveWorkspace(_) => unreachable!("workspace requests detach first"),
            RequestFrame::PrepareAgentResources(request) => {
                self.prepare_agent_resources(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::AddWorktree(request) => {
                self.add_worktree(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::RemoveWorktree(request) => {
                self.remove_worktree(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::InstallAgentSkill(request) => {
                self.install_agent_skill(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::MaterializeAgentHome(request) => {
                self.materialize_agent_home(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::RemoveAgentHome(request) => {
                self.remove_agent_home(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::OpenAgent(request) => {
                self.open_agent(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::CloseAgent(request) => {
                self.close_agent(request.authority_fence, request.request)
                    .await
            }
            RequestFrame::CaptureAgent(reference) => {
                safe_component(reference.agent_id.as_str(), "agent id")?;
                let home = self.config.agent_home(reference.agent_id.as_str());
                if !path_is_directory(&home) {
                    return Err(RuntimeError::rejected(
                        "agent_home_not_found",
                        "agent home was not found",
                    ));
                }
                Ok(ResponseFrame::CaptureAgent(crate::conversation::capture(
                    &home,
                )?))
            }
            RequestFrame::Checkpoint(request) => self.checkpoint(&request).await,
            RequestFrame::CheckpointForTeardown(request) => {
                self.checkpoint_for_teardown(request).await
            }
            RequestFrame::ResumeCheckpointWriters(request) => {
                self.resume_checkpoint_writers(request).await
            }
            RequestFrame::Restore(request) => self.restore(&request).await,
            RequestFrame::Resources => Ok(ResponseFrame::Resources(
                self.resources.sample(std::time::Instant::now()).into(),
            )),
            RequestFrame::ProveAgentProcessClosed(_)
            | RequestFrame::FenceAgentRestart(_)
            | RequestFrame::WorkspaceRemovalSupport(_)
            | RequestFrame::ReconcileWorkspaceRemoval(_)
            | RequestFrame::RuntimeStatus(_)
            | RequestFrame::ToolProxyStatus(_) => {
                unreachable!("status requests observe without mutation")
            }
        }
    }
}
