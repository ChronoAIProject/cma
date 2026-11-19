//! Refresh tools on an idle retained thread using pinned, existing ACP methods.
use crate::ingress_budget::diagnostic::text;
use std::sync::atomic::Ordering;

use super::acp::{Bridge, CALL_DEADLINE};
use crate::{AdapterError, PermissionLevel, ReasoningEffort};

#[derive(serde::Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub(super) enum ContextRequest<'a> {
    Close {
        session_id: &'a str,
    },
    Load {
        session_id: &'a str,
        cwd: &'a str,
        mcp_servers: &'a serde_json::Value,
    },
}

#[path = "acp_context_refresh_mode.rs"]
mod mode;

struct RefreshRequest<'a> {
    session: &'a str,
    operation_sequence: u64,
    readiness_receipt: &'a str,
    guidance: &'a str,
    previous_guidance: &'a [String],
    model: &'a str,
    effort: ReasoningEffort,
    permission: PermissionLevel,
    working_directory: Option<&'a str>,
    service_origin: Option<&'a str>,
}

impl Bridge {
    pub(super) async fn context_readiness_receipt(
        &self,
        sequence: u64,
        transport: bool,
    ) -> Option<String> {
        let session = self.session().await.ok()?;
        if sequence == 0 || self.context_busy().await {
            return None;
        }
        let binding = self.context_refresh.as_ref()?;
        let observed = self
            .context_goal
            .lock()
            .await
            .as_ref()
            .map(|goal| goal.is_safe_for(&session));
        match observed {
            Some(true) => {}
            None if transport && binding.transport_goal_absent(&session).await.is_ok() => {}
            _ => return None,
        }
        binding.readiness_receipt(&session, sequence).await.ok()
    }

    /// Inspect only this attachment's local state. Receipt issuance and all
    /// runtime IO remain behind the caller's fresh admission checks.
    pub(super) async fn context_readiness_session(&self) -> Option<String> {
        self.context_refresh.as_ref()?;
        let session = self.session().await.ok()?;
        if self.context_busy().await
            || !self
                .context_goal
                .lock()
                .await
                .as_ref()
                .is_some_and(|goal| goal.is_safe_for(&session))
        {
            return None;
        }
        Some(session)
    }

    pub(super) async fn context_busy(&self) -> bool {
        self.owned_prompt_waiters.load(Ordering::SeqCst) != 0
            || self.orphaned_prompt_response.load(Ordering::SeqCst)
            || !self.blocked.lock().await.is_empty()
            || self.pending.lock().await.values().any(|call| call.prompt)
    }

    pub(super) async fn observe_context_frame_goal(&self, frame: &super::retained_json::JsonFrame) {
        if let Some(observation) = super::context_goal::Observation::read(frame) {
            observation.record(&mut *self.context_goal.lock().await);
        }
    }

    #[cfg(test)]
    pub(super) async fn observe_context_goal(&self, session: &str, update: &serde_json::Value) {
        let frame = super::retained_json::JsonFrame::legacy(serde_json::json!({
            "params": {"sessionId": session, "update": update},
        }));
        self.observe_context_frame_goal(&frame).await;
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn refresh_context(
        &self,
        session: &str,
        operation_sequence: u64,
        readiness_receipt: &str,
        guidance: &str,
        previous_guidance: &[String],
        model: &str,
        effort: ReasoningEffort,
        permission: PermissionLevel,
        working_directory: Option<&str>,
        service_origin: Option<&str>,
    ) -> crate::Result<()> {
        self.refresh_context_with(
            RefreshRequest {
                session,
                operation_sequence,
                readiness_receipt,
                guidance,
                previous_guidance,
                model,
                effort,
                permission,
                working_directory,
                service_origin,
            },
            &mode::Legacy,
        )
        .await
    }

    pub(super) async fn drive_admitted(
        &self,
        input: crate::ConversationInput,
        updates: &super::update_relay::UpdateRelay,
    ) -> Result<(), crate::ingress_budget::diagnostic::OperationError> {
        let crate::ConversationInput::RefreshContext {
            service_origin,
            operation_sequence,
            readiness_receipt,
            previous_guidance,
            working_directory,
            guidance,
            model,
            reasoning_effort,
            permission,
        } = &input
        else {
            return self.drive(input, updates).await.map_err(Into::into);
        };
        let _session_storage = self.input_storage(&input)?;
        let session = self.session().await?;
        let policy =
            self.ingress
                .as_ref()
                .ok_or(crate::ingress_budget::AdmittedError::Unsupported(
                    "admitted refresh requires an ownership policy",
                ))?;
        self.refresh_context_with(
            RefreshRequest {
                session: &session,
                operation_sequence: *operation_sequence,
                readiness_receipt,
                guidance,
                previous_guidance,
                model,
                effort: *reasoning_effort,
                permission: *permission,
                working_directory: working_directory.as_deref(),
                service_origin: service_origin.as_deref(),
            },
            &mode::Admitted(policy),
        )
        .await?;
        self.recovering_context.store(0, Ordering::SeqCst);
        Ok(())
    }

    async fn refresh_context_with<M: mode::Mode>(
        &self,
        request: RefreshRequest<'_>,
        mode: &M,
    ) -> Result<(), M::Error> {
        let RefreshRequest {
            session,
            operation_sequence: _,
            readiness_receipt: _,
            guidance: _,
            previous_guidance: _,
            model,
            effort,
            permission,
            working_directory,
            service_origin,
        } = request;
        if self.context_busy().await
            || self
                .context_goal
                .lock()
                .await
                .as_ref()
                .is_some_and(|goal| !goal.is_safe_for(session))
        {
            return Err(AdapterError::OperationIdentityConflict(text(
                "conversation must settle before managed context refresh",
            ))
            .into());
        }
        let binding = self.context_refresh.as_ref().ok_or_else(|| {
            AdapterError::Unsupported(text("runtime has no retained context writer"))
        })?;
        let cwd = working_directory.unwrap_or_else(|| binding.cwd());
        if !cwd.starts_with('/') || cwd.len() > 4096 || cwd.contains('\0') || model.is_empty() {
            return Err(
                AdapterError::Unsupported(text("retained context identity is invalid")).into(),
            );
        }
        let transport = mode.prepare(binding, &request).await?;
        if service_origin.is_some() && self.context_goal.lock().await.is_none() {
            mode.goal_absent(binding, session).await?;
        }
        let tools = binding.tools_frame_for_transport(transport.as_ref(), self.ingress.as_ref())?;
        mode.append(binding, &request).await?;
        // The native app-server retains MCP configuration for a live thread.
        // Unsubscribe first, then load its persisted ID. This neither stops the
        // sandbox process nor creates a new conversation or home.
        let refreshed = async {
            if self.context_busy().await
                || self
                    .context_goal
                    .lock()
                    .await
                    .as_ref()
                    .is_some_and(|goal| !goal.is_safe_for(session))
            {
                return Err(AdapterError::OperationIdentityConflict(text(
                    "conversation became busy before managed context reload",
                ))
                .into());
            }
            self.call_prepared(
                "session/close",
                &ContextRequest::Close {
                    session_id: session,
                },
                CALL_DEADLINE,
            )
            .await?;
            let loaded = self
                .load_retained_prepared(&ContextRequest::Load {
                    session_id: session,
                    cwd,
                    mcp_servers: &tools,
                })
                .await?;
            if loaded
                .get("sessionId")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|id| id != session)
            {
                return Err(AdapterError::Rejected(text(
                    "managed refresh loaded another conversation",
                ))
                .into());
            }
            self.configure_settings(session, model, effort, permission, None)
                .await?;
            if !self.matches_active_session(session).await? {
                return Err(AdapterError::Rejected(text(
                    "managed refresh changed conversation identity",
                ))
                .into());
            }
            if let Some(transport) = &transport {
                mode.confirm(binding, transport, &request).await?;
            }
            Ok(())
        }
        .await;
        refreshed.map_err(|_error: M::Error| {
            // Native MCP failures can echo descriptors carrying credentials.
            AdapterError::Unavailable(text("managed context refresh is not confirmed")).into()
        })
    }
}
