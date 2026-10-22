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
        if let Some(prompt) = system_prompt {
            use sha2::{Digest, Sha256};
            let answer = self
                .call(
                    "cma/agent-instructions",
                    serde_json::json!({
                        "action": "apply", "sessionId": session, "systemPrompt": prompt,
                    }),
                )
                .await?;
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
