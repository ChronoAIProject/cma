use super::*;

impl PublicationStore {
    pub(super) async fn change(&self, id: &AgentId, change: Change) {
        let mut agent = self.inner.get_agent(id).await.unwrap().unwrap();
        let revision = agent.revision;
        match change {
            Change::None | Change::Intent | Change::Material | Change::Settings => return,
            Change::Expire => {
                tokio::time::advance(Duration::from_secs(31)).await;
                return;
            }
            Change::OwnerReplacement => {
                use layer2_core::agent_ownership::OwnershipStore;
                let authority = layer2_core::agent_ownership::execution_authority().unwrap();
                let store = self.ownership.as_ref().unwrap();
                store.release(&authority.token).await.unwrap();
                let replacement = store
                    .acquire(id, &authority.token.instance_id)
                    .await
                    .unwrap()
                    .unwrap();
                assert!(replacement.token.epoch > authority.token.epoch);
                assert!(
                    authority.is_current(),
                    "durable replacement must be detected independently of local revocation"
                );
                return;
            }
            Change::LeaseDeadline => {
                agent.work.lease.as_mut().unwrap().expires_at =
                    rfc3339_at(SystemTime::now() + Duration::from_secs(60))
            }
            Change::PreferenceTitle => {
                use layer2_core::{Authorization, Caller, Capability};
                AgentOperationService::new(
                    self.inner.clone(),
                    Arc::new(LiveAgentRegistry::default()),
                )
                .update_preferences(
                    &Caller::new(
                        agent.owner.clone(),
                        Authorization::new([Capability::UseOwnSandboxes]),
                    ),
                    id,
                    layer2_core::sandbox::AgentPreferencesUpdate::Title {
                        expected_revision: agent.conversation.preferences.revision,
                        title: "Concurrent owner override".into(),
                    },
                )
                .await
                .unwrap();
                return;
            }
            Change::Title => agent.title = Some("Concurrent owner title".into()),
            Change::Log => {
                self.inner
                    .append_agent_log(
                        id,
                        agent.updated_at.clone(),
                        AgentLogKind::TitleSet {
                            title: "Concurrent transcript title".into(),
                            source: "owner".into(),
                        },
                    )
                    .await
                    .unwrap();
                agent.conversation.last_seq += 1;
            }
            Change::Holder => agent.work.lease.as_mut().unwrap().holder = "other-AM".into(),
            Change::Fence => {
                agent.work.lease.as_mut().unwrap().fence += 1;
                agent.work.next_fence += 1;
            }
            Change::Step => agent.work.step = AgentWorkStep::CloseProcess,
            Change::Runtime => {
                agent.conversation.runtime_generation = Some(99);
                agent.conversation.conversation_id = Some("other-conversation".into());
            }
            Change::Stop | Change::Delete => {
                self.inner
                    .schedule_agent_stop(AgentStopRequest {
                        target: AgentControlTarget::from(&agent),
                        reason: AgentStopReason::Owner,
                        requested_at: agent.updated_at.clone(),
                    })
                    .await
                    .unwrap();
                if matches!(change, Change::Delete) {
                    let mut stopped = self.inner.get_agent(id).await.unwrap().unwrap();
                    let revision = stopped.revision;
                    stopped
                        .transition(
                            AgentStatus::Stopped,
                            stopped.updated_at.clone(),
                            Some(AgentStopReason::Owner),
                        )
                        .unwrap();
                    stopped
                        .work
                        .schedule(AgentWorkStep::None, stopped.updated_at.clone());
                    self.inner
                        .save_agent(revision, stopped.clone())
                        .await
                        .unwrap();
                    self.inner
                        .schedule_agent_delete(AgentDeleteRequest {
                            target: AgentControlTarget::from(&stopped),
                            requested_at: stopped.updated_at.clone(),
                        })
                        .await
                        .unwrap();
                }
                return;
            }
            Change::ParentPause | Change::ParentRuntime => {
                let mut parent = self
                    .inner
                    .get_sandbox(&agent.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = parent.revision;
                if matches!(change, Change::ParentPause) {
                    parent.begin_pause(agent.updated_at.clone(), None).unwrap();
                } else {
                    parent.runtime.as_mut().unwrap().runtime_id = "replacement-runtime".into();
                    parent.revision += 1;
                }
                self.inner.save_sandbox(revision, parent).await.unwrap();
                return;
            }
            Change::Workspace => {
                let mut workspace = self
                    .inner
                    .get_workspace(&agent.workspace_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = workspace.revision;
                workspace.resolved_commit = Some("replacement-commit".into());
                workspace.revision += 1;
                self.inner
                    .save_workspace(revision, workspace)
                    .await
                    .unwrap();
                return;
            }
            Change::Worktree | Change::WorktreeRevision => {
                let mut worktree = self
                    .inner
                    .get_worktree(&agent.worktree_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = worktree.revision;
                if matches!(change, Change::Worktree) {
                    worktree.base_commit = "replacement-worktree-base".into();
                }
                worktree.revision += 1;
                self.inner.save_worktree(revision, worktree).await.unwrap();
                return;
            }
        }
        agent.revision += 1;
        self.inner.save_agent(revision, agent).await.unwrap();
    }
}
