use super::acp::Bridge;
use crate::{AdapterError, PermissionLevel, ReasoningEffort};

impl Bridge {
    pub(super) async fn configure_settings(
        &self,
        session: &str,
        model: &str,
        effort: ReasoningEffort,
        permission: PermissionLevel,
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
}
