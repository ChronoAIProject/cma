use super::acp::{Bridge, PROTOCOL_VERSION, SIGN_IN_DEADLINE};
use super::update_relay::UpdateRelay;
use super::HostedAgentOpenSettings;
use crate::AdapterError;

enum Greeting {
    Fresh(serde_json::Value),
    Existing,
}

impl Bridge {
    pub(super) async fn handshake(
        &self,
        resume: Option<String>,
        cwd: String,
        settings: HostedAgentOpenSettings,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        let protected = settings.channel_tool.is_some() || settings.provider_tool.is_some();
        self.handshake_inner(resume, cwd, settings, updates)
            .await
            .map_err(|error| {
                if protected {
                    protected_setup_failure(error)
                } else {
                    error
                }
            })
    }

    async fn handshake_inner(
        &self,
        resume: Option<String>,
        cwd: String,
        settings: HostedAgentOpenSettings,
        updates: &UpdateRelay,
    ) -> crate::Result<()> {
        validate_settings(&cwd, &settings)?;
        let greeting = self
            .greet(serde_json::json!({
                "protocolVersion": PROTOCOL_VERSION,
                "clientCapabilities": {
                    "fs": { "readTextFile": false, "writeTextFile": false },
                    "elicitation": { "url": {} },
                    // Codex ACP otherwise emits provider failures as assistant text
                    // followed by end_turn. Negotiate its structured failure result.
                    "_meta": {
                        "jetbrains": { "air": { "version": 1, "capabilities": ["sessionFailure"] } }
                    },
                },
            }),settings.channel_tool.is_some() || settings.provider_tool.is_some())
            .await?;

        if let Some(binding) = &settings.channel_tool {
            match &greeting {
                Greeting::Fresh(hello)
                    if hello.pointer("/agentCapabilities/mcpCapabilities/http")
                        == Some(&serde_json::Value::Bool(true)) => {}
                Greeting::Existing
                    if resume.as_deref().is_some_and(|id| binding.retained_for(id)) => {}
                _ => {
                    return Err(AdapterError::Unsupported(
                        "runtime cannot establish the required channel reply tool connection"
                            .into(),
                    ))
                }
            }
        }
        if let Some(binding) = &settings.provider_tool {
            match &greeting {
                Greeting::Fresh(hello)
                    if binding.observed().is_some()
                        && hello.pointer("/agentCapabilities/mcpCapabilities/http")
                            == Some(&serde_json::Value::Bool(true)) => {}
                Greeting::Existing
                    if resume.as_deref().is_some_and(|id| binding.retained_for(id)) => {}
                _ => {
                    return Err(AdapterError::Unsupported(
                        "runtime cannot establish the required provider tools".into(),
                    ))
                }
            }
        }
        let tool_servers: Vec<_> = settings
            .channel_tool
            .as_ref()
            .map(|binding| binding.acp_server())
            .into_iter()
            .chain(
                settings
                    .provider_tool
                    .as_ref()
                    .map(|binding| binding.acp_server()),
            )
            .collect();

        if settings.own_credential && self.subscription.is_some() {
            self.call("cma/codex/authenticate", serde_json::json!({}))
                .await?;
        } else if settings.own_credential
            && resume.is_none()
            && matches!(greeting, Greeting::Fresh(_))
        {
            let Greeting::Fresh(hello) = &greeting else {
                unreachable!("fresh greeting was matched")
            };
            self.authenticate_owner(hello).await?;
        }

        let session = match (greeting, resume) {
            (Greeting::Fresh(_), Some(conversation)) => {
                self.activate_session(conversation.clone(), updates).await?;
                self.call(
                    "session/load",
                    serde_json::json!({
                        "sessionId": conversation,
                        "cwd": cwd,
                        "mcpServers": tool_servers,
                    }),
                )
                .await
                .map_err(|error| error.context("could not resume conversation"))?;
                conversation
            }
            (Greeting::Fresh(_), None) => {
                let started = self
                    .call(
                        "session/new",
                        serde_json::json!({ "cwd": cwd, "mcpServers": tool_servers }),
                    )
                    .await?;
                let conversation = started
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| AdapterError::Rejected("bridge named no conversation".into()))?
                    .to_string();
                self.activate_session(conversation.clone(), updates).await?;
                conversation
            }
            (Greeting::Existing, Some(conversation)) => {
                self.activate_session(conversation.clone(), updates).await?;
                tracing::info!("rejoining the conversation held by the runtime");
                conversation
            }
            (Greeting::Existing, None) => {
                return Err(AdapterError::Rejected(
                    "an initialized bridge requires the held conversation identifier".into(),
                ));
            }
        };

        if let Some(mode) = settings.permission_mode {
            self.call(
                "session/set_mode",
                serde_json::json!({ "sessionId": session, "modeId": mode }),
            )
            .await
            .map_err(|error| {
                error.context(&format!(
                    "conversation could not be held to permission mode {mode}"
                ))
            })?;
        }
        if settings.channel_tool.is_some() {
            // adapter_agent accepts this opaque binding only from the trusted
            // channel source after durable creation-provenance and exact-open
            // checks. Manual/Trigger Agents cannot opt in through runtime flags.
            // Apply on new/load/rejoin so an existing Agent also benefits, without
            // changing its immutable profile, home or launch request identity.
            self.configure_channel_effort(&session).await;
        }
        Ok(())
    }

    async fn configure_channel_effort(&self, session: &str) {
        let result = self
            .call_within(
                "session/set_config_option",
                serde_json::json!({
                    "sessionId": session, "configId": "reasoning_effort", "value": "low"
                }),
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

    async fn greet(&self, params: serde_json::Value, protected: bool) -> crate::Result<Greeting> {
        match self.call("initialize", params).await {
            Ok(hello) => Ok(Greeting::Fresh(hello)),
            Err(refused @ AdapterError::Rejected(_)) => {
                if protected {
                    tracing::info!("channel runtime bridge was already initialized");
                } else {
                    tracing::info!(%refused, "runtime bridge was already initialized");
                }
                Ok(Greeting::Existing)
            }
            Err(error) => Err(error),
        }
    }

    async fn authenticate_owner(&self, hello: &serde_json::Value) -> crate::Result<()> {
        let methods = hello
            .get("authMethods")
            .and_then(serde_json::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let chosen = methods
            .iter()
            .filter_map(|method| method.get("id").and_then(serde_json::Value::as_str))
            .find(|id| id.contains("device"))
            .ok_or_else(|| {
                AdapterError::Unsupported(
                    "runtime offers no remotely completable owner sign-in".into(),
                )
            })?
            .to_string();
        self.call_within(
            "authenticate",
            serde_json::json!({ "methodId": chosen }),
            SIGN_IN_DEADLINE,
        )
        .await
        .map_err(|error| error.context("owner sign-in did not complete"))?;
        Ok(())
    }
}

/// MCP setup carries a limited CMA bearer. Runtimes may echo that descriptor in
/// error JSON; preserve only the category before worker logs retain the failure.
fn protected_setup_failure(error: AdapterError) -> AdapterError {
    let detail = "channel tool session setup did not complete".to_string();
    match error {
        AdapterError::Unavailable(_) => AdapterError::Unavailable(detail),
        AdapterError::Rejected(_) => AdapterError::Rejected(detail),
        AdapterError::OperationIdentityConflict(_) => {
            AdapterError::OperationIdentityConflict(detail)
        }
        AdapterError::AtCapacity(_) => AdapterError::AtCapacity(detail),
        AdapterError::Unsupported(_) => AdapterError::Unsupported(detail),
        AdapterError::RuntimeContractMismatch(_) => AdapterError::Rejected(detail),
    }
}

fn validate_settings(cwd: &str, settings: &HostedAgentOpenSettings) -> crate::Result<()> {
    let valid_mode = settings.permission_mode.as_deref().is_none_or(|mode| {
        !mode.is_empty()
            && mode.len() <= 128
            && mode.trim() == mode
            && !mode.chars().any(char::is_control)
    });
    if cwd.is_empty()
        || cwd.len() > 4_096
        || !cwd.starts_with('/')
        || cwd.chars().any(char::is_control)
        || !valid_mode
    {
        return Err(AdapterError::Rejected(
            "ACP working directory or permission mode is invalid".into(),
        ));
    }
    Ok(())
}
