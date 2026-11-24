use layer3_adapter::wire::{FencedReceipt, RequestFrame, ResponseFrame};
use layer3_adapter::{WorktreeReceipt, WorktreeRef, WorktreeRemoveReceipt, WorktreeRequest};

use super::{agent_validation, Supervisor};
use crate::failure::{Result, RuntimeError};
use crate::mutation_journal::{Operation, Subject};
use crate::mutation_store::{finish, Begin, Cursor, Ticket};

impl Supervisor {
    pub(super) async fn authorize_mutation(&self, request: &RequestFrame) -> Result<()> {
        if request.has_unexpected_restart_runtime() {
            return Err(RuntimeError::rejected(
                "agent_process_restart_runtime_unexpected",
                "only a numbered Agent Open accepts a restart runtime identity",
            ));
        }
        if let RequestFrame::PrepareAgentResources(request) = request {
            return self.authorize_agent_resources(request).await;
        }
        let fenced = match request {
            RequestFrame::AddWorktree(request) => {
                crate::worktree::validate_add_request(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::add_worktree(&request.request),
                ))
            }
            RequestFrame::RemoveWorktree(request) => {
                crate::worktree::validate_remove_request(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::remove_worktree(&request.request),
                ))
            }
            RequestFrame::InstallAgentSkill(request) => {
                crate::agent_skill_install::validate(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::install_agent_skill(&request.request),
                ))
            }
            RequestFrame::MaterializeAgentHome(request) => {
                agent_validation::materialize_home(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::create_agent_home(&request.request),
                ))
            }
            RequestFrame::RemoveAgentHome(request) => {
                agent_validation::remove_home(&request.request)?;
                Some((
                    request.authority_fence,
                    Operation::remove_agent_home(&request.request),
                ))
            }
            RequestFrame::OpenAgent(request) => Some((
                request.authority_fence,
                self.authorize_process_open(request).await?,
            )),
            RequestFrame::CloseAgent(request) => Some((
                request.authority_fence,
                self.authorize_process_close(&request.request).await?,
            )),
            _ => None,
        };
        let Some((authority_fence, operation)) = fenced else {
            return Ok(());
        };
        self.authorize_preparation(request).await?;
        self.mutations.admit(authority_fence, &[operation]).await
    }

    pub(super) async fn add_worktree(
        &self,
        authority_fence: u64,
        request: WorktreeRequest,
    ) -> Result<ResponseFrame> {
        crate::worktree::validate_add_request(&request)?;
        let operation = Operation::add_worktree(&request);
        if !self.mutations.admitted(&operation).await? {
            crate::worktree::preflight_add(
                &self.config.home,
                self.config.allow_local_git,
                &request,
            )
            .await?;
        }
        match self.mutations.begin::<WorktreeReceipt>(&operation).await? {
            Begin::Completed(receipt) => Ok(add_response(authority_fence, receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_add_worktree(authority_fence, request, ticket)
                    .await
            }
        }
    }

    async fn execute_add_worktree(
        &self,
        authority_fence: u64,
        request: WorktreeRequest,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        match crate::worktree::inspect_add(&self.config.home, self.config.allow_local_git, &request)
            .await
        {
            Ok(Some(receipt)) => {
                finish(ticket, Ok(receipt), |receipt| {
                    add_response(authority_fence, receipt)
                })
                .await
            }
            Ok(None) => {
                match crate::worktree::add(&self.config.home, self.config.allow_local_git, &request)
                    .await
                {
                    Ok(receipt) => {
                        finish(ticket, Ok(receipt), |receipt| {
                            add_response(authority_fence, receipt)
                        })
                        .await
                    }
                    Err(error) => {
                        self.finish_failed_add_worktree(authority_fence, &request, ticket, error)
                            .await
                    }
                }
            }
            Err(error) if error.retryable => Err(error),
            Err(error) => {
                tracing::warn!(detail = %error, "worktree addition recovery failed closed");
                Err(ticket.indeterminate(
                    "worktree addition left partial or mismatched Git state; explicit recovery is required",
                ).await?)
            }
        }
    }

    async fn finish_failed_add_worktree(
        &self,
        authority_fence: u64,
        request: &WorktreeRequest,
        ticket: Ticket,
        error: crate::failure::RuntimeError,
    ) -> Result<ResponseFrame> {
        match crate::worktree::inspect_add(&self.config.home, self.config.allow_local_git, request)
            .await
        {
            Ok(Some(receipt)) => {
                finish(ticket, Ok(receipt), |receipt| {
                    add_response(authority_fence, receipt)
                })
                .await
            }
            Ok(None) if error.retryable => Err(error),
            Ok(None) => {
                ticket.fail(&error).await?;
                Err(error)
            }
            Err(observation) if observation.retryable => Err(observation),
            Err(observation) => {
                tracing::warn!(
                    original = %error,
                    detail = %observation,
                    "worktree addition outcome is indeterminate"
                );
                Err(ticket.indeterminate(
                    "worktree addition left partial or mismatched Git state; explicit recovery is required",
                ).await?)
            }
        }
    }

    pub(super) async fn remove_worktree(
        &self,
        authority_fence: u64,
        request: WorktreeRef,
    ) -> Result<ResponseFrame> {
        crate::worktree::validate_remove_request(&request)?;
        let operation = Operation::remove_worktree(&request);
        let path =
            crate::worktree::expected_path(&self.config.home, &request.workspace, &request.name);
        self.ensure_worktree_idle(&path).await?;
        if !self.mutations.admitted(&operation).await? {
            crate::worktree::preflight_remove(
                &self.config.home,
                self.config.allow_local_git,
                &request,
            )
            .await?;
        }
        match self
            .mutations
            .begin::<WorktreeRemoveReceipt>(&operation)
            .await?
        {
            Begin::Completed(receipt) => Ok(remove_response(authority_fence, receipt)),
            Begin::Failed(error) | Begin::Indeterminate(error) => Err(error),
            Begin::New(ticket) | Begin::Started(ticket) => {
                self.execute_remove_worktree(authority_fence, request, ticket)
                    .await
            }
        }
    }

    async fn execute_remove_worktree(
        &self,
        authority_fence: u64,
        request: WorktreeRef,
        ticket: Ticket,
    ) -> Result<ResponseFrame> {
        match crate::worktree::remove(&self.config.home, self.config.allow_local_git, &request)
            .await
        {
            Ok(()) => {
                finish(
                    ticket,
                    Ok(WorktreeRemoveReceipt {
                        operation_id: request.operation_id,
                        request_sha256: request.request_sha256,
                        workspace: request.workspace,
                        name: request.name,
                    }),
                    |receipt| remove_response(authority_fence, receipt),
                )
                .await
            }
            Err(error) => Err(crate::failure::RuntimeError::unavailable(
                "worktree_remove_incomplete",
                format!("worktree removal has not converged: {error}"),
            )),
        }
    }

    pub(super) async fn recover_mutations_locked(&self) -> Result<()> {
        let mut cursor = Cursor::default();
        loop {
            let page = self.mutations.started(&mut cursor).await?;
            if page.is_empty() {
                break;
            }
            for pending in page {
                let Some(authority_fence) =
                    self.mutations.recovery_fence(&pending.operation).await?
                else {
                    pending.ticket.supersede().await?;
                    continue;
                };
                match pending.operation.subject.clone() {
                    Subject::AddWorktree {
                        workspace,
                        name,
                        base,
                    } => {
                        let request = WorktreeRequest {
                            operation_id: pending.operation.operation_id.clone(),
                            request_sha256: pending.operation.request_sha256.clone(),
                            workspace,
                            name,
                            base,
                        };
                        let (ticket, terminal) = pending.ticket.track_terminal();
                        terminal.absorb(
                            self.execute_add_worktree(authority_fence, request, ticket)
                                .await,
                        )?;
                    }
                    Subject::RemoveWorktree { workspace, name } => {
                        let request = WorktreeRef {
                            operation_id: pending.operation.operation_id.clone(),
                            request_sha256: pending.operation.request_sha256.clone(),
                            workspace,
                            name,
                        };
                        let (ticket, terminal) = pending.ticket.track_terminal();
                        terminal.absorb(
                            self.execute_remove_worktree(authority_fence, request, ticket)
                                .await,
                        )?;
                    }
                    Subject::InstallAgentSkill { request } => {
                        let (ticket, terminal) = pending.ticket.track_terminal();
                        terminal.absorb(
                            self.execute_install_agent_skill(authority_fence, request, ticket)
                                .await,
                        )?;
                    }
                    Subject::CreateAgentHome { .. } => self.recover_agent_home(pending).await?,
                    Subject::OpenAgent { .. } => self.recover_agent_open(pending).await?,
                    Subject::CloseAgent { .. } => {
                        self.recover_agent_close(authority_fence, pending).await?
                    }
                    Subject::RemoveAgentHome { .. } => {
                        self.recover_agent_home_removal(pending).await?
                    }
                }
            }
        }
        Ok(())
    }
}

fn add_response(authority_fence: u64, receipt: WorktreeReceipt) -> ResponseFrame {
    ResponseFrame::AddWorktree(FencedReceipt::new(authority_fence, receipt))
}

fn remove_response(authority_fence: u64, receipt: WorktreeRemoveReceipt) -> ResponseFrame {
    ResponseFrame::RemoveWorktree(FencedReceipt::new(authority_fence, receipt))
}
