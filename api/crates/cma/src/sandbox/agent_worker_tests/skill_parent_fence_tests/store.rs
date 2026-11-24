use super::*;
use layer2_core::sandbox::store::AgentWorkDiscovery;

pub(super) struct ProbeStore {
    pub inner: Arc<MongoSandboxStore>,
    pub cleanup_worker: Arc<AgentWorker>,
    pub admin: Client,
    pub app: String,
    pub case: Case,
    pub attempts: AtomicUsize,
    pub observation: Arc<Mutex<fixture::Observation>>,
    pub ownership: Option<Arc<MongoAgentOwnership>>,
    pub proposed: Mutex<Option<AgentWorkMutation>>,
    pub before: Mutex<Option<Agent>>,
}

impl ProbeStore {
    async fn interleave(&self, mutation: &AgentWorkMutation) {
        match self.case {
            Case::KnownAbort | Case::AbortThenPause => {
                self.admin
                    .database("admin")
                    .run_command(doc! {
                        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
                        "data":{"failCommands":["update"],"appName":&self.app,
                            "errorCode":112_i32,"errorLabels":["TransientTransactionError"]}
                    })
                    .await
                    .unwrap();
            }
            Case::UnknownCommitted | Case::FailedReadback => {
                self.admin
                    .database("admin")
                    .run_command(doc! {
                        "configureFailPoint":"failCommand", "mode":{"times":5_i32},
                        "data":{"failCommands":["commitTransaction"],"appName":&self.app,
                            "writeConcernError":{"code":50_i32,"codeName":"MaxTimeMSExpired",
                                "errmsg":"injected unknown skill publication"}}
                    })
                    .await
                    .unwrap();
            }
            Case::ParentPause | Case::ParentRuntime => {
                let mut parent = self
                    .inner
                    .get_sandbox(&mutation.agent.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                if matches!(self.case, Case::ParentPause) {
                    parent
                        .begin_pause(rfc3339_at(SystemTime::now()), None)
                        .unwrap();
                } else {
                    parent.runtime.as_mut().unwrap().runtime_id = "replacement-runtime".into();
                    parent.revision += 1;
                }
                self.inner
                    .save_sandbox(revision, parent.clone())
                    .await
                    .unwrap();
                assert_eq!(
                    self.inner.get_sandbox(&parent.id).await.unwrap(),
                    Some(parent)
                );
                eprintln!("SKILL_FENCE_PARENT_INTERLEAVE durable parent replacement completed before publication transaction");
            }
            Case::WorkspaceRemoving | Case::WorktreeRemoving => {
                crate::store::skill_result_test_support::change_parent(
                    &self.inner,
                    &mutation.agent,
                    if matches!(self.case, Case::WorkspaceRemoving) {
                        "workspace"
                    } else {
                        "worktree"
                    },
                )
                .await;
            }
            Case::MissingSandbox | Case::MissingWorkspace | Case::MissingWorktree => {
                let which = match self.case {
                    Case::MissingSandbox => "sandbox",
                    Case::MissingWorkspace => "workspace",
                    _ => "worktree",
                };
                crate::store::skill_result_test_support::remove_parent(
                    &self.inner,
                    &mutation.agent,
                    which,
                )
                .await;
            }
            Case::Metadata => {
                let mut parent = self
                    .inner
                    .get_sandbox(&mutation.agent.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                parent.revision += 1;
                parent.last_activity_at = rfc3339_at(SystemTime::now() + Duration::from_secs(1));
                self.inner.save_sandbox(revision, parent).await.unwrap();
                let mut workspace = self
                    .inner
                    .get_workspace(&mutation.agent.workspace_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = workspace.revision;
                workspace.revision += 1;
                workspace.next_worktree_number += 1;
                self.inner
                    .save_workspace(revision, workspace)
                    .await
                    .unwrap();
                let mut tree = self
                    .inner
                    .get_worktree(&mutation.agent.worktree_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = tree.revision;
                tree.revision += 1;
                self.inner.save_worktree(revision, tree).await.unwrap();
            }
            Case::Valid
            | Case::UnprovedBoundary
            | Case::Refusal
            | Case::SnapshotSandbox
            | Case::SnapshotWorkspace
            | Case::SnapshotWorktree
            | Case::RollbackLateReceipt => {}
            Case::Stop | Case::Delete => {
                let agent = self
                    .inner
                    .get_agent(&mutation.agent.id)
                    .await
                    .unwrap()
                    .unwrap();
                self.inner
                    .schedule_agent_stop(AgentStopRequest {
                        target: AgentControlTarget::from(&agent),
                        reason: AgentStopReason::Owner,
                        requested_at: rfc3339_at(SystemTime::now()),
                    })
                    .await
                    .unwrap();
                if matches!(self.case, Case::Delete) {
                    let cleanup_at = SystemTime::UNIX_EPOCH
                        + Duration::from_secs(
                            layer2_core::seconds_from_rfc3339(
                                &agent.work.lease.as_ref().unwrap().expires_at,
                            )
                            .unwrap(),
                        );
                    for step in 0..8 {
                        if self
                            .inner
                            .get_agent(&agent.id)
                            .await
                            .unwrap()
                            .unwrap()
                            .status
                            == AgentStatus::Stopped
                        {
                            break;
                        }
                        self.cleanup_worker
                            .run_one(&agent.id, cleanup_at + Duration::from_secs(step))
                            .await
                            .unwrap();
                    }
                    let stopped = self.inner.get_agent(&agent.id).await.unwrap().unwrap();
                    assert_eq!(
                        stopped.status,
                        AgentStatus::Stopped,
                        "actual cleanup must finish before Delete admission"
                    );
                    self.inner
                        .schedule_agent_delete(AgentDeleteRequest {
                            target: AgentControlTarget::from(&stopped),
                            requested_at: stopped.updated_at.clone(),
                        })
                        .await
                        .unwrap();
                }
            }
            Case::Owner => {
                let authority = layer2_core::agent_ownership::execution_authority().unwrap();
                let owner = self.ownership.as_ref().unwrap();
                owner.release(&authority.token).await.unwrap();
                let successor = owner
                    .acquire(&mutation.agent.id, &authority.token.instance_id)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(successor.token.epoch > authority.token.epoch);
                assert!(
                    authority.is_current(),
                    "local authority intentionally remains unrecalled"
                );
            }
        }
    }
}

#[async_trait::async_trait]
impl AgentWorkerStore for ProbeStore {
    async fn due_agent_work_filtered(
        &self,
        now: &str,
        limit: u16,
        filter: AgentWorkDiscovery<'_>,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.inner.due_agent_work_filtered(now, limit, filter).await
    }
    async fn live_agent_connections(
        &self,
        page: &PageRequest,
    ) -> Result<Page<AgentConnectionRecoveryCandidate>, SandboxStoreError> {
        self.inner.live_agent_connections(page).await
    }
    async fn schedule_agent_connection_recovery(
        &self,
        request: AgentConnectionRecoveryRequest,
    ) -> Result<AgentConnectionRecoveryOutcome, SandboxStoreError> {
        self.inner.schedule_agent_connection_recovery(request).await
    }
    async fn due_agent_work(
        &self,
        now: &str,
        limit: u16,
    ) -> Result<Vec<AgentId>, SandboxStoreError> {
        self.inner.due_agent_work(now, limit).await
    }
    async fn claim_agent_work(
        &self,
        id: &AgentId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Agent>, SandboxStoreError> {
        self.inner
            .claim_agent_work(id, holder, now, lease_expires_at)
            .await
    }
    async fn commit_agent_work(
        &self,
        mutation: AgentWorkMutation,
    ) -> Result<(), SandboxStoreError> {
        self.inner.commit_agent_work(mutation).await
    }
    async fn commit_agent_skill_result(
        &self,
        mutation: AgentWorkMutation,
        guard: AgentSkillResultGuard,
    ) -> Result<(), SandboxStoreError> {
        let terminal = mutation.agent_logs.iter().any(|log| {
            matches!(
                log,
                AgentLogKind::SkillInstallationChanged {
                    status: AgentSkillInstallStatus::Installed | AgentSkillInstallStatus::Failed,
                    ..
                }
            )
        });
        if terminal {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                let before = self.inner.get_agent(&mutation.agent.id).await?.unwrap();
                *self.before.lock().unwrap() = Some(before);
                *self.proposed.lock().unwrap() = Some(mutation.clone());
                self.observation.lock().unwrap().armed = true;
                self.interleave(&mutation).await;
            }
        }
        match self.case {
            Case::SnapshotSandbox | Case::SnapshotWorkspace | Case::SnapshotWorktree => {
                let which = match self.case {
                    Case::SnapshotSandbox => "sandbox",
                    Case::SnapshotWorkspace => "workspace",
                    _ => "worktree",
                };
                return crate::store::skill_result_test_support::snapshot_conflict(
                    &self.inner,
                    &mutation,
                    &guard,
                    which,
                )
                .await;
            }
            Case::RollbackLateReceipt => {
                return crate::store::skill_result_test_support::rollback_and_reconcile_after_pause(
                    &self.inner,
                    &mutation,
                    &guard,
                )
                .await
            }
            _ => {}
        }
        if matches!(self.case, Case::UnprovedBoundary) {
            let before = crate::store::skill_result_test_support::snapshot(&self.inner).await;
            assert_eq!(
                self.inner.commit_agent_work(mutation.clone()).await,
                Err(SandboxStoreError::Conflict)
            );
            assert_eq!(
                crate::store::skill_result_test_support::snapshot(&self.inner).await,
                before
            );
        }
        let before = crate::store::skill_result_test_support::snapshot(&self.inner).await;
        let result = self
            .inner
            .commit_agent_skill_result(mutation.clone(), guard)
            .await;
        if matches!(
            result,
            Err(SandboxStoreError::Conflict | SandboxStoreError::NotFound)
        ) {
            assert_eq!(
                crate::store::skill_result_test_support::snapshot(&self.inner).await,
                before,
                "refusal must not leave parent fence, Agent, log or projection changes"
            );
        }
        if matches!(self.case, Case::AbortThenPause) {
            assert_eq!(result, Err(SandboxStoreError::Conflict));
            crate::store::skill_result_test_support::change_parent(
                &self.inner,
                &mutation.agent,
                "sandbox",
            )
            .await;
        }
        result
    }
}
#[path = "forward.rs"]
mod forward;
