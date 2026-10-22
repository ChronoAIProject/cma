use super::acp::Bridge;
use crate::{AdapterError, PermissionLevel, ReasoningEffort};

impl Bridge {
    pub(super) async fn configure_settings(
        &self,
        session: &str,
        model: &str,
        effort: ReasoningEffort,
        permission: PermissionLevel,
        system_prompt: Option<&str>,
    ) -> crate::Result<()> {
        if model.is_empty()
            || model.len() > 256
            || model.trim() != model
            || model.chars().any(char::is_control)
        {
            return Err(AdapterError::Unsupported(
                "invalid conversation model".into(),
            ));
        }
        if let Some(prompt) = system_prompt {
            use crate::ConversationSettingsSupport;
            if prompt.len() > 32 * 1024
                || prompt.contains('\0')
                || !self.system_prompt_supported().await
            {
                return Err(AdapterError::Unsupported(
                    "this runtime cannot update Agent instructions; update the Sandbox when idle"
                        .into(),
                ));
            }
        }
        self.configure_choices(session, model, effort, permission)
            .await?;
        if let Some(prompt) = system_prompt {
            use sha2::{Digest, Sha256};
            let request = serde_json::json!({
                "action": "apply", "sessionId": session, "systemPrompt": prompt,
            });
            let answer = match self.call("cma/agent-instructions", request.clone()).await {
                Err(error) if legacy_instruction_resume_failed(&error) => {
                    let recovered = async {
                        self.reload_failed_instructions(session, prompt).await?;
                        self.configure_choices(session, model, effort, permission)
                            .await?;
                        // Replay the same absolute desired value once, under
                        // the original operation and runtime identity.
                        self.call("cma/agent-instructions", request).await
                    }
                    .await;
                    // The first apply already changed the file. No subsequent
                    // refusal can be classified as a pre-write failure.
                    recovered.map_err(|error: AdapterError| {
                        AdapterError::Unavailable(format!(
                            "pending Agent instructions are not confirmed: {error}"
                        ))
                    })?
                }
                result => result?,
            };
            let expected = format!("{:x}", Sha256::digest(prompt.as_bytes()));
            if answer.get("applied").and_then(serde_json::Value::as_bool) != Some(true)
                || answer.get("sessionId").and_then(serde_json::Value::as_str) != Some(session)
                || answer.get("sha256").and_then(serde_json::Value::as_str)
                    != Some(expected.as_str())
            {
                return Err(AdapterError::Rejected(
                    "runtime did not confirm Agent instructions".into(),
                ));
            }
        }
        Ok(())
    }

    async fn configure_choices(
        &self,
        session: &str,
        model: &str,
        effort: ReasoningEffort,
        permission: PermissionLevel,
    ) -> crate::Result<()> {
        // Use acknowledged requests; a successful channel write means all three
        // settings were accepted. An uncertain/partial result closes the writer
        // and the durable desired settings are reapplied before reattachment.
        // The pinned codex-acp 1.4.0 legacy model method requires the combined
        // model[effort] form and validates both before changing session state.
        self.call(
            "session/set_model",
            serde_json::json!({
                "sessionId": session, "modelId": format!("{model}[{}]", effort.as_str()),
            }),
        )
        .await
        .map_err(|error| match error {
            // The pinned method validates the full model/effort pair before its
            // single state change. A JSON-RPC refusal has no configuration effect.
            AdapterError::Rejected(_) => AdapterError::Unsupported(
                "runtime does not support the requested model and reasoning effort".into(),
            ),
            other => other,
        })?;
        let response = self
            .call(
                "session/set_config_option",
                serde_json::json!({
                    "sessionId": session, "configId": "reasoning_effort", "value": effort.as_str(),
                }),
            )
            .await?;
        let confirmed = response
            .get("configOptions")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|options| {
                options.iter().any(|option| {
                    option.get("id").and_then(serde_json::Value::as_str) == Some("reasoning_effort")
                        && option
                            .get("currentValue")
                            .and_then(serde_json::Value::as_str)
                            == Some(effort.as_str())
                })
            });
        if !confirmed {
            return Err(AdapterError::Rejected(
                "runtime did not confirm requested reasoning effort".into(),
            ));
        }
        let mode = match permission {
            PermissionLevel::Guarded => "read-only",
            PermissionLevel::Autonomous => "agent-full-access",
        };
        self.call(
            "session/set_mode",
            serde_json::json!({
                "sessionId": session, "modeId": mode,
            }),
        )
        .await?;
        Ok(())
    }

    async fn reload_failed_instructions(&self, session: &str, prompt: &str) -> crate::Result<()> {
        self.require_instruction_recovery_idle(session).await?;
        let binding = self.context_refresh.as_ref().ok_or_else(|| {
            AdapterError::Unsupported("runtime has no retained instructions identity".into())
        })?;
        binding.confirm_instructions(prompt).await?;
        let mut parameters = self.retained_instruction_session(session).await?;
        parameters["mcpServers"] = serde_json::json!(binding.tools());
        self.require_instruction_recovery_idle(session).await?;
        // This exact legacy failure occurs after the native idle check, atomic
        // instruction write and native unsubscribe, before resume can start.
        // Load the same persisted thread with full bound MCP definitions. Never
        // close/cancel here: recovery must not interrupt unrelated native work.
        let loaded = self.call("session/load", parameters).await?;
        if loaded
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id != session)
            || self.session().await? != session
        {
            return Err(AdapterError::Rejected(
                "instructions recovery changed conversation identity".into(),
            ));
        }
        Ok(())
    }

    async fn require_instruction_recovery_idle(&self, session: &str) -> crate::Result<()> {
        if self.context_busy().await
            || self.session().await? != session
            || self
                .context_goal
                .lock()
                .await
                .as_ref()
                .is_some_and(|(id, safe)| id != session || !safe)
        {
            return Err(AdapterError::OperationIdentityConflict(
                "conversation must settle before instructions can recover".into(),
            ));
        }
        Ok(())
    }

    async fn retained_instruction_session(
        &self,
        session: &str,
    ) -> crate::Result<serde_json::Value> {
        let mut cursor = None::<String>;
        for _ in 0..8 {
            let page = self
                .call("session/list", serde_json::json!({"cursor": cursor}))
                .await?;
            if let Some(found) = page
                .get("sessions")
                .and_then(serde_json::Value::as_array)
                .and_then(|sessions| {
                    sessions.iter().find(|value| {
                        value.get("sessionId").and_then(serde_json::Value::as_str) == Some(session)
                    })
                })
            {
                let cwd = found
                    .get("cwd")
                    .and_then(serde_json::Value::as_str)
                    .filter(|cwd| cwd.starts_with('/') && cwd.len() <= 4096 && !cwd.contains('\0'))
                    .ok_or_else(|| {
                        AdapterError::Rejected("retained instructions directory unavailable".into())
                    })?;
                let mut parameters = serde_json::json!({"sessionId": session, "cwd": cwd});
                if let Some(directories) = found.get("additionalDirectories") {
                    parameters["additionalDirectories"] = directories.clone();
                }
                return Ok(parameters);
            }
            let next = page.get("nextCursor").and_then(serde_json::Value::as_str);
            if next.is_none() || next == cursor.as_deref() {
                break;
            }
            cursor = next.map(str::to_owned);
        }
        Err(AdapterError::Rejected(
            "pending instructions conversation was not found".into(),
        ))
    }
}

fn legacy_instruction_resume_failed(error: &AdapterError) -> bool {
    let AdapterError::Rejected(detail) = error else {
        return false;
    };
    let Some(json) = detail.strip_prefix("cma/agent-instructions: ") else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return false;
    };
    value
        == serde_json::json!({
            "code": -32603, "message": "Internal error",
            "data": {"details": "Cannot read properties of undefined (reading 'replace')"},
        })
}

#[async_trait::async_trait]
impl crate::ConversationSettingsSupport for Bridge {
    async fn managed_context_receipt(&self, sequence: u64) -> Option<String> {
        self.context_readiness_receipt(sequence).await
    }

    async fn system_prompt_supported(&self) -> bool {
        use std::sync::atomic::Ordering;
        match self.instructions_support.load(Ordering::Acquire) {
            1 => return true,
            2 => return false,
            _ => {}
        }
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            self.call(
                "cma/agent-instructions",
                serde_json::json!({"action":"support"}),
            ),
        )
        .await;
        match result {
            Ok(Ok(value)) => {
                let supported = value.get("supported").and_then(serde_json::Value::as_bool)
                    == Some(true)
                    && value.get("version").and_then(serde_json::Value::as_u64) == Some(1);
                self.instructions_support
                    .store(if supported { 1 } else { 2 }, Ordering::Release);
                supported
            }
            Ok(Err(AdapterError::Rejected(_))) => {
                self.instructions_support.store(2, Ordering::Release);
                false
            }
            _ => false,
        }
    }
}
