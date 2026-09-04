//! Chat-thread allocation over one durable shared host.

use std::sync::Arc;

use layer3_adapter::{RuntimeHandle, WorkspaceTreeRequest};

use crate::authz::{Caller, Capability};
use crate::capabilities::SessionCapabilities;
use crate::clock::now_rfc3339;
use crate::event::{Event, EventKind};
use crate::options::{AgentOptions, CredentialMode, PermissionMode, SessionMode};
use crate::service::{store_failed, SessionService};
use crate::session::{ChatSession, RecoveryState, Session, SessionSpec, SessionStatus};
use crate::store::{ChatOrigin, SessionRecord, SkillPackState};
use crate::{
    ChatHostId, ChatHostRecord, ChatHostStatus, ChatProvisioningPhase, ChatThreadSlot,
    ChatUnavailableCause, CoreError, SessionId,
};

fn unavailable(cause: ChatUnavailableCause) -> CoreError {
    CoreError::ChatUnavailable(cause)
}

fn host_store_unavailable() -> CoreError {
    unavailable(ChatUnavailableCause::HostStoreUnavailable)
}

fn thread_failure(retryable: bool) -> CoreError {
    unavailable(ChatUnavailableCause::ProvisioningFailed {
        phase: ChatProvisioningPhase::WorkspaceSource,
        command_index: None,
        retryable,
    })
}

/// Compose the exact text delivered to a chat runtime.
///
/// Prefix validation belongs to deployment settings. This boundary only
/// preserves their configured order and adds the one separating space each
/// prefix contributes before the owner's text.
pub(crate) fn compose_chat_message(prefixes: &[String], text: &str) -> String {
    if prefixes.is_empty() {
        return text.to_owned();
    }

    let mut effective = prefixes.join(" ");
    effective.push(' ');
    effective.push_str(text);
    effective
}

impl SessionService {
    /// Resolve a prompt against the immutable settings snapshot that owns its
    /// chat thread. Ordinary sessions are deliberately an exact identity.
    pub(crate) async fn effective_prompt_text(
        &self,
        record: &SessionRecord,
        text: &str,
    ) -> Result<String, CoreError> {
        let Some(origin) = record.chat_origin.as_ref() else {
            return Ok(text.to_owned());
        };
        let store = self
            .chat_hosts
            .as_ref()
            .ok_or_else(host_store_unavailable)?;
        let host = store
            .get(&origin.host_id)
            .await
            .map_err(|_| host_store_unavailable())?
            .ok_or_else(|| unavailable(ChatUnavailableCause::Retired))?;
        if host.owner != record.session.owner {
            return Err(CoreError::Forbidden);
        }
        if host.status != ChatHostStatus::Active {
            return Err(unavailable(ChatUnavailableCause::Provisioning));
        }
        let retained = host.threads.iter().any(|slot| {
            slot.session_id == record.session.id
                && slot.worktree_name == origin.worktree_name
                && slot.worktree_path == origin.worktree_path
                && !slot.removed
        });
        if !retained {
            return Err(CoreError::RuntimeUnavailable(
                "the chat host no longer contains this thread allocation".into(),
            ));
        }

        Ok(compose_chat_message(
            &host.pinned_settings.settings.message_prefixes,
            text,
        ))
    }

    /// Create one conversation and worktree on the owner's shared chat host.
    pub(crate) async fn create_chat_thread(
        self: &Arc<Self>,
        caller: &Caller,
        spec: SessionSpec,
    ) -> Result<Session, CoreError> {
        caller.authorization.require(Capability::CreateSession)?;
        if spec.environment_profile.is_some() {
            return Err(CoreError::InvalidRequest(
                "surface chat cannot be combined with environment_profile".into(),
            ));
        }

        self.revive_chat_host_for_new_thread(&caller.subject)
            .await?;
        let ensured = self
            .ensure_chat_host(&caller.subject, spec.caller_credential.as_ref())
            .await?;
        let store = self.chat_hosts.clone().ok_or_else(host_store_unavailable)?;
        let _owner_guard = self.foreground_chat_host_guard(&caller.subject).await;

        let session_id = SessionId::generate();
        let (host, worktree_name) = loop {
            let mut host = store
                .get(&ensured.id)
                .await
                .map_err(|_| host_store_unavailable())?
                .ok_or_else(|| unavailable(ChatUnavailableCause::Retired))?;
            match host.status {
                ChatHostStatus::Active => {}
                ChatHostStatus::Paused | ChatHostStatus::Pausing | ChatHostStatus::Reviving => {
                    return Err(unavailable(ChatUnavailableCause::RevivePending));
                }
                ChatHostStatus::Retired => {
                    return Err(unavailable(ChatUnavailableCause::Retired));
                }
                ChatHostStatus::Provisioning => {
                    return Err(unavailable(ChatUnavailableCause::Provisioning));
                }
            }
            if host.last_transition_claim.is_some() {
                return Err(unavailable(ChatUnavailableCause::Provisioning));
            }
            if host.owner != caller.subject {
                return Err(CoreError::Forbidden);
            }
            let live = host.threads.iter().filter(|thread| !thread.removed).count() as u32;
            let limit = u32::from(host.pinned_settings.settings.host.max_threads);
            if live >= limit {
                return Err(CoreError::ChatThreadLimit { limit, live });
            }
            host.threads_created = host.threads_created.checked_add(1).ok_or_else(|| {
                CoreError::InvalidRequest("the chat thread counter is exhausted".into())
            })?;
            let worktree_name = format!("chat-{}", host.threads_created);
            host.threads.push(ChatThreadSlot {
                session_id: session_id.clone(),
                worktree_name: worktree_name.clone(),
                // Reserved before the provider resolves its absolute path.
                worktree_path: String::new(),
                created_at: now_rfc3339(),
                removed: false,
            });
            host.updated_at = now_rfc3339();
            match store
                .update_if_revision(host.revision, &host)
                .await
                .map_err(|_| host_store_unavailable())?
            {
                Some(committed) => break (committed, worktree_name),
                None => continue,
            }
        };

        let runtime_id = host
            .runtime_id
            .clone()
            .ok_or_else(|| unavailable(ChatUnavailableCause::Provisioning))?;
        let adapter = self.adapters.get(&host.kernel).cloned().ok_or_else(|| {
            unavailable(ChatUnavailableCause::CapabilityUnsupported(
                "configured chat kernel".into(),
            ))
        })?;
        let request = WorkspaceTreeRequest {
            name: worktree_name.clone(),
            base: host.pinned_settings.settings.workspace.branch.clone(),
        };
        let receipt = match adapter
            .add_workspace_tree(&RuntimeHandle(runtime_id.clone()), &request)
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                self.abandon_chat_thread(
                    &host.id,
                    &session_id,
                    &worktree_name,
                    Some((&runtime_id, &adapter)),
                )
                .await;
                return Err(thread_failure(!matches!(
                    error,
                    layer3_adapter::AdapterError::Unsupported(_)
                )));
            }
        };

        let host = match self
            .record_chat_worktree_path(&host.id, &session_id, &receipt.path)
            .await
        {
            Ok(host) => host,
            Err(error) => {
                self.abandon_chat_thread(
                    &host.id,
                    &session_id,
                    &worktree_name,
                    Some((&runtime_id, &adapter)),
                )
                .await;
                return Err(error);
            }
        };
        let origin = ChatOrigin {
            host_id: host.id.clone(),
            worktree_name: worktree_name.clone(),
            worktree_path: receipt.path,
            title: None,
            paused_notice: None,
        };
        let chat = origin.public_projection();
        let settings = &host.pinned_settings.settings.agent;
        let options = AgentOptions {
            mode: SessionMode::Conversation,
            model: settings.model.clone(),
            reasoning_effort: settings.reasoning_effort,
            permission: PermissionMode::Unrestricted,
            credential: CredentialMode::Delegated,
            size: None,
            record_tool_bodies: settings.record_tool_bodies,
        };
        let capabilities =
            SessionCapabilities::granted(SessionMode::Conversation, adapter.capabilities())?;
        let baseline = self.environments.baseline();
        let session = Session {
            options,
            chat: Some(ChatSession { ..chat }),
            id: session_id.clone(),
            owner: caller.subject.clone(),
            kernel: host.kernel.clone(),
            status: SessionStatus::Ready,
            recovery_state: RecoveryState::Normal,
            stop_reason: None,
            geometry: spec.geometry.unwrap_or_default(),
            last_seq: 0,
            environment_profile: None,
            runtime_version: host.runtime_version.clone(),
            rotations: 0,
            rotated_at: None,
            capabilities,
            granted_services: Some(baseline.services().to_vec()),
            granted_skills: Some(
                baseline
                    .skills()
                    .iter()
                    .cloned()
                    .map(|id| crate::GrantedSkill { id, version: None })
                    .collect(),
            ),
            pricing: None,
            created_at: now_rfc3339(),
        };
        let mut record = SessionRecord::new(session.clone());
        record.runtime_id = Some(runtime_id.clone());
        record.chat_origin = Some(origin);
        record.skill_packs = SkillPackState::Cleaned;
        let created = self
            .create_durably(
                &mut record,
                vec![
                    Event {
                        session_id: session_id.clone(),
                        seq: 0,
                        kind: EventKind::SessionCreated,
                        appended_at: String::new(),
                    },
                    Event {
                        session_id: session_id.clone(),
                        seq: 0,
                        kind: EventKind::StatusChanged {
                            status: SessionStatus::Ready,
                            stop_reason: None,
                            detail: None,
                            capability: None,
                        },
                        appended_at: String::new(),
                    },
                ],
            )
            .await;
        if let Err(error) = created {
            self.sequences.forget(&session_id);
            self.abandon_chat_thread(
                &host.id,
                &session_id,
                &worktree_name,
                Some((&runtime_id, &adapter)),
            )
            .await;
            return Err(store_failed(error));
        }

        let opened = self.readopt(&record).await;
        let credential = if opened.is_ok() {
            self.install_chat_host_credential(&host.id, spec.caller_credential.as_ref())
                .await
        } else {
            Ok(())
        };
        if let Err(error) = opened.and(credential) {
            self.entries.write().await.remove(&session_id);
            self.abandon_chat_thread(
                &host.id,
                &session_id,
                &worktree_name,
                Some((&runtime_id, &adapter)),
            )
            .await;
            return Err(error);
        }

        self.store
            .get(&session_id)
            .await
            .map_err(store_failed)?
            .map(|record| record.session)
            .ok_or(CoreError::SessionNotFound)
    }

    async fn record_chat_worktree_path(
        &self,
        host_id: &ChatHostId,
        session_id: &SessionId,
        path: &str,
    ) -> Result<ChatHostRecord, CoreError> {
        let store = self
            .chat_hosts
            .as_ref()
            .ok_or_else(host_store_unavailable)?;
        loop {
            let mut host = store
                .get(host_id)
                .await
                .map_err(|_| host_store_unavailable())?
                .ok_or_else(|| unavailable(ChatUnavailableCause::Retired))?;
            let slot = host
                .threads
                .iter_mut()
                .find(|thread| &thread.session_id == session_id && !thread.removed)
                .ok_or_else(|| unavailable(ChatUnavailableCause::Provisioning))?;
            if !slot.worktree_path.is_empty() && slot.worktree_path != path {
                return Err(unavailable(ChatUnavailableCause::Provisioning));
            }
            slot.worktree_path = path.to_string();
            host.updated_at = now_rfc3339();
            match store
                .update_if_revision(host.revision, &host)
                .await
                .map_err(|_| host_store_unavailable())?
            {
                Some(committed) => return Ok(committed),
                None => continue,
            }
        }
    }

    /// Best-effort rollback of one exact failed allocation. The empty active
    /// host remains retryable; failed creation never retires the host.
    async fn abandon_chat_thread(
        &self,
        host_id: &ChatHostId,
        session_id: &SessionId,
        worktree_name: &str,
        runtime: Option<(&str, &Arc<dyn layer3_adapter::KernelAdapter>)>,
    ) {
        if let Some((runtime_id, adapter)) = runtime {
            if let Err(error) = adapter
                .remove_workspace_tree(&RuntimeHandle(runtime_id.to_string()), worktree_name)
                .await
            {
                tracing::warn!(
                    chat_host = %host_id,
                    session_id = %session_id,
                    worktree = %worktree_name,
                    %error,
                    "failed chat-thread worktree cleanup remains visible on its host slot"
                );
            }
        }
        if let Some(store) = self.chat_hosts.as_ref() {
            loop {
                let Ok(Some(mut host)) = store.get(host_id).await else {
                    break;
                };
                let Some(slot) = host
                    .threads
                    .iter_mut()
                    .find(|thread| &thread.session_id == session_id)
                else {
                    break;
                };
                if slot.removed {
                    break;
                }
                slot.removed = true;
                host.updated_at = now_rfc3339();
                match store.update_if_revision(host.revision, &host).await {
                    Ok(Some(_)) => break,
                    Ok(None) => continue,
                    Err(_) => break,
                }
            }
        }
        if self.store.get(session_id).await.ok().flatten().is_some() {
            if let Err(error) = self.delete_durably(session_id).await {
                tracing::warn!(
                    session_id = %session_id,
                    %error,
                    "failed chat-thread session rollback remains durable for reconciliation"
                );
            } else {
                self.sequences.forget(session_id);
            }
        }
    }
}
