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
            )));
        }
        let binding = self.context_refresh.as_ref().ok_or_else(|| {
            AdapterError::Unsupported(text("runtime has no retained context writer"))
        })?;
        let cwd = working_directory.unwrap_or_else(|| binding.cwd());
        if !cwd.starts_with('/') || cwd.len() > 4096 || cwd.contains('\0') || model.is_empty() {
            return Err(AdapterError::Unsupported(text(
                "retained context identity is invalid",
            )));
        }
        let transport = binding
            .prepare_transport(session, operation_sequence, service_origin)
            .await?;
        if service_origin.is_some() && self.context_goal.lock().await.is_none() {
            binding.transport_goal_absent(session).await?;
        }
        let tools = binding.tools_frame_for_transport(transport.as_ref(), self.ingress.as_ref())?;
        binding
            .append_guidance(
                session,
                operation_sequence,
                readiness_receipt,
                guidance,
                previous_guidance,
            )
            .await?;
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
                )));
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
                )));
            }
            self.configure_settings(session, model, effort, permission, None)
                .await?;
            if !self.matches_active_session(session).await? {
                return Err(AdapterError::Rejected(text(
                    "managed refresh changed conversation identity",
                )));
            }
            if let Some(transport) = &transport {
                binding
                    .confirm_transport(transport, session, operation_sequence)
                    .await?;
            }
            Ok(())
        }
        .await;
        refreshed.map_err(|_error: AdapterError| {
            // Native MCP failures can echo descriptors carrying credentials.
            AdapterError::Unavailable(text("managed context refresh is not confirmed"))
        })
    }
}
