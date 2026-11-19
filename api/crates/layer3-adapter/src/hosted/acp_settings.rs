use super::acp::{Bridge, CALL_DEADLINE};
use super::retained_json::JsonFrame;
use crate::{AdapterError, PermissionLevel, ReasoningEffort};

#[derive(serde::Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub(super) enum SettingsRequest<'a> {
    List {
        cursor: Option<&'a str>,
    },
    Support {
        action: &'static str,
    },
    Model {
        session_id: &'a str,
        model_id: ModelChoice<'a>,
    },
    Effort {
        session_id: &'a str,
        config_id: &'static str,
        value: &'static str,
    },
    Mode {
        session_id: &'a str,
        mode_id: &'static str,
    },
    Instructions {
        action: &'static str,
        session_id: &'a str,
        system_prompt: &'a str,
    },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RetainedInstructions<'a> {
    session_id: &'a str,
    cwd: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    additional_directories: Option<&'a serde_json::Value>,
}

#[derive(serde::Serialize)]
struct ReloadInstructions<'a> {
    #[serde(flatten)]
    session: &'a serde_json::Value,
    #[serde(rename = "mcpServers")]
    tools: &'a serde_json::Value,
}

pub(super) struct ModelChoice<'a>(pub(super) &'a str, pub(super) ReasoningEffort);

impl std::fmt::Display for ModelChoice<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(out, "{}[{}]", self.0, self.1.as_str())
    }
}

impl serde::Serialize for ModelChoice<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The sizing serializer counts formatting growth before to_value can
        // create the combined model identifier.
        serializer.collect_str(self)
    }
}

impl Bridge {
    pub(super) async fn configure_channel_effort(&self, session: &str) {
        let result = self
            .call_prepared(
                "session/set_config_option",
                &SettingsRequest::Effort {
                    session_id: session,
                    config_id: "reasoning_effort",
                    value: "low",
                },
                std::time::Duration::from_secs(1),
            )
            .await;
        let confirmed = result.as_ref().is_ok_and(|response| {
            response
                .get("configOptions")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|options| {
                    options.iter().any(|option| {
                        option.get("id").and_then(serde_json::Value::as_str)
                            == Some("reasoning_effort")
                            && option
                                .get("currentValue")
                                .and_then(serde_json::Value::as_str)
                                == Some("low")
                            && option
                                .get("options")
                                .and_then(serde_json::Value::as_array)
                                .is_some_and(|values| {
                                    values.iter().any(|value| {
                                        value.get("value").and_then(serde_json::Value::as_str)
                                            == Some("low")
                                    })
                                })
                    })
                })
        });
        if confirmed {
            tracing::info!("channel session reasoning effort set to low");
        } else {
            // Effort is a latency preference, never delivery authority. An old
            // runtime or unsupported value must not prevent the input opening.
            // Do not retain remote error bodies or echoed session settings.
            tracing::warn!(
                "channel session reasoning effort was not confirmed; retaining runtime setting"
            );
        }
    }

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
            let request = SettingsRequest::Instructions {
                action: "apply",
                session_id: session,
                system_prompt: prompt,
            };
            let answer = match self
                .call_prepared("cma/agent-instructions", &request, CALL_DEADLINE)
                .await
            {
                Err(error)
                    if legacy_instruction_resume_failed(
                        &error,
                        self.json_pools.as_ref().filter(|_| self.ingress.is_some()),
                    )? =>
                {
                    let recovered = async {
                        self.reload_failed_instructions(session, prompt).await?;
                        self.configure_choices(session, model, effort, permission)
                            .await?;
                        // Replay the same absolute desired value once, under
                        // the original operation and runtime identity.
                        self.call_prepared("cma/agent-instructions", &request, CALL_DEADLINE)
                            .await
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
            let _digest_storage = self
                .ingress
                .as_ref()
                .map(|policy| {
                    policy.reserve(
                        crate::ingress_budget::IngressClass::Data,
                        crate::ingress_budget::AllocationPlan::new(
                            "instruction confirmation digest",
                            &[3 * 64],
                        )?,
                    )
                })
                .transpose()?;
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
        self.call_prepared(
            "session/set_model",
            &SettingsRequest::Model {
                session_id: session,
                model_id: ModelChoice(model, effort),
            },
            CALL_DEADLINE,
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
            .call_prepared(
                "session/set_config_option",
                &SettingsRequest::Effort {
                    session_id: session,
                    config_id: "reasoning_effort",
                    value: effort.as_str(),
                },
                CALL_DEADLINE,
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
        self.call_prepared(
            "session/set_mode",
            &SettingsRequest::Mode {
                session_id: session,
                mode_id: mode,
            },
            CALL_DEADLINE,
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
        let parameters = self.retained_instruction_session(session).await?;
        let tools = binding.tools_frame(self.ingress.as_ref())?;
        self.require_instruction_recovery_idle(session).await?;
        // This exact legacy failure occurs after the native idle check, atomic
        // instruction write and native unsubscribe, before resume can start.
        // Load the same persisted thread with full bound MCP definitions. Never
        // close/cancel here: recovery must not interrupt unrelated native work.
        let loaded = self
            .load_retained_prepared(&ReloadInstructions {
                session: &parameters,
                tools: &tools,
            })
            .await?;
        if loaded
            .get("sessionId")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|id| id != session)
            || !self.matches_active_session(session).await?
        {
            return Err(AdapterError::Rejected(
                "instructions recovery changed conversation identity".into(),
            ));
        }
        Ok(())
    }

    async fn require_instruction_recovery_idle(&self, session: &str) -> crate::Result<()> {
        if self.context_busy().await
            || !self.matches_active_session(session).await?
            || self
                .context_goal
                .lock()
                .await
                .as_ref()
                .is_some_and(|goal| !goal.is_safe_for(session))
        {
            return Err(AdapterError::OperationIdentityConflict(
                "conversation must settle before instructions can recover".into(),
            ));
        }
        Ok(())
    }

    pub(super) async fn retained_instruction_session(
        &self,
        session: &str,
    ) -> crate::Result<JsonFrame> {
        let mut cursor = None::<JsonFrame>;
        for _ in 0..8 {
            let page = self
                .call_prepared(
                    "session/list",
                    &SettingsRequest::List {
                        cursor: cursor.as_ref().and_then(|cursor| cursor.as_str()),
                    },
                    CALL_DEADLINE,
                )
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
                return JsonFrame::prepare(
                    &RetainedInstructions {
                        session_id: session,
                        cwd,
                        additional_directories: found.get("additionalDirectories"),
                    },
                    self.ingress.as_ref(),
                );
            }
            let next = page.get("nextCursor").and_then(serde_json::Value::as_str);
            if next.is_none() || next == cursor.as_ref().and_then(|cursor| cursor.as_str()) {
                break;
            }
            cursor = next
                .map(|next| JsonFrame::prepare(&next, self.ingress.as_ref()))
                .transpose()?;
        }
        Err(AdapterError::Rejected(
            "pending instructions conversation was not found".into(),
        ))
    }
}

pub(super) fn legacy_instruction_resume_failed(
    error: &AdapterError,
    pools: Option<&super::retained_json::JsonPools>,
) -> crate::Result<bool> {
    let AdapterError::Rejected(detail) = error else {
        return Ok(false);
    };
    let Some(json) = detail.strip_prefix("cma/agent-instructions: ") else {
        return Ok(false);
    };
    let value = match pools {
        Some(pools) => match pools.decode(json.as_bytes()) {
            Ok(value) => value,
            Err(AdapterError::Rejected(_)) => return Ok(false),
            // Apply may already have changed the file. Capacity cannot turn
            // this uncertain result into a pre-write refusal.
            Err(_) => {
                return Err(AdapterError::Unavailable(
                    "pending Agent instructions recovery inspection at capacity".into(),
                ));
            }
        },
        None => match serde_json::from_str(json) {
            Ok(value) => JsonFrame::legacy(value),
            Err(_) => return Ok(false),
        },
    };
    Ok(value.as_object().is_some_and(|fields| fields.len() == 3)
        && value.get("code").and_then(serde_json::Value::as_i64) == Some(-32603)
        && value.get("message").and_then(serde_json::Value::as_str) == Some("Internal error")
        && value
            .get("data")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|data| {
                data.len() == 1
                    && data.get("details").and_then(serde_json::Value::as_str)
                        == Some("Cannot read properties of undefined (reading 'replace')")
            }))
}

#[async_trait::async_trait]
impl crate::ConversationSettingsSupport for Bridge {
    async fn delivery_observation(&self) -> crate::ingress_budget::DeliveryObservation {
        crate::ingress_budget::DeliveryObservation {
            progress: self.output_observation().await,
            history: Some(self.retained_history_deadline.observe()),
        }
    }
    async fn managed_context_may_be_ready(&self) -> bool {
        self.context_readiness_session().await.is_some()
    }
    async fn retained_history_deadline(
        &self,
    ) -> Option<tokio::sync::watch::Receiver<Option<tokio::time::Instant>>> {
        self.retained_history_deadline.legacy_receiver()
    }
    async fn managed_context_progress(&self) -> Option<tokio::sync::watch::Receiver<u64>> {
        self.output_progress().await
    }

    async fn managed_context_receipt(&self, sequence: u64) -> Option<String> {
        self.context_readiness_receipt(sequence, false).await
    }

    async fn managed_transport_receipt(&self, sequence: u64) -> Option<String> {
        self.context_readiness_receipt(sequence, true).await
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
            self.call_prepared(
                "cma/agent-instructions",
                &SettingsRequest::Support { action: "support" },
                CALL_DEADLINE,
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
