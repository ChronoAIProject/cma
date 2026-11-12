use super::acp::{
    Bridge, CALL_DEADLINE, PROTOCOL_VERSION, RETAINED_HISTORY_IDLE_DEADLINE, SIGN_IN_DEADLINE,
};
use super::update_relay::UpdateRelay;
use super::HostedAgentOpenSettings;
use crate::AdapterError;

enum Greeting {
    Fresh(super::retained_json::JsonFrame),
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
        let protected = settings.channel_tool.is_some()
            || settings.provider_tool.is_some()
            || settings.access_tool.is_some();
        let mut phase = "settings_validation";
        self.handshake_inner(resume, cwd, settings, updates, &mut phase)
            .await
            .map_err(|error| {
                if protected {
                    protected_setup_failure(error, phase)
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
        phase: &mut &'static str,
    ) -> crate::Result<()> {
        validate_settings(&cwd, &settings)?;
        let mut timing = super::startup_timing::StartupTiming::new();
        *phase = "protocol_initialize";
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
            }), settings.already_initialized_error.as_ref(), resume.is_some())
            .await?;
        timing.completed("protocol_initialize");

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
            .chain(
                settings
                    .access_tool
                    .as_ref()
                    .map(|binding| binding.acp_server()),
            )
            .collect();
        *phase = "open_completion";
        let completion = if let (Greeting::Existing, Some(conversation), Some(protocol)) =
            (&greeting, resume.as_deref(), settings.open_completion)
        {
            let missing_confirmation = settings
                .channel_tool
                .as_ref()
                .is_some_and(|binding| !binding.retained_for(conversation))
                || settings
                    .provider_tool
                    .as_ref()
                    .is_some_and(|binding| !binding.retained_for(conversation))
                || settings.access_tool.as_ref().is_some_and(|binding| {
                    !binding.retained_for(conversation) && !binding.prepared_for(conversation)
                });
            if missing_confirmation {
                let digest = super::acp_open_completion::binding_digest(&cwd, &tool_servers)?;
                Some((
                    protocol,
                    self.read_open_completion(protocol, conversation, &digest)
                        .await?,
                ))
            } else {
                None
            }
        } else {
            None
        };
        let completed = completion.is_some();

        *phase = if matches!(greeting, Greeting::Existing) {
            "channel_tool_confirmation"
        } else {
            "channel_tool_capability"
        };
        if let Some(binding) = &settings.channel_tool {
            match &greeting {
                Greeting::Fresh(hello)
                    if hello.pointer("/agentCapabilities/mcpCapabilities/http")
                        == Some(&serde_json::Value::Bool(true)) => {}
                Greeting::Existing
                    if resume
                        .as_deref()
                        .is_some_and(|id| binding.retained_for(id) || completed) => {}
                Greeting::Existing => return Err(unconfirmed_setup()),
                _ => {
                    return Err(AdapterError::Unsupported(
                        "runtime cannot establish the required channel reply tool connection"
                            .into(),
                    ))
                }
            }
        }
        *phase = if matches!(greeting, Greeting::Existing) {
            "provider_tool_confirmation"
        } else {
            "provider_tool_capability"
        };
        if let Some(binding) = &settings.provider_tool {
            match &greeting {
                Greeting::Fresh(hello)
                    if binding.observed().is_some()
                        && hello.pointer("/agentCapabilities/mcpCapabilities/http")
                            == Some(&serde_json::Value::Bool(true)) => {}
                Greeting::Existing
                    if binding.observed().is_some()
                        && resume
                            .as_deref()
                            .is_some_and(|id| binding.retained_for(id) || completed) => {}
                Greeting::Existing => return Err(unconfirmed_setup()),
                _ => {
                    return Err(AdapterError::Unsupported(
                        "runtime cannot establish the required provider tools".into(),
                    ))
                }
            }
        }
        *phase = if matches!(greeting, Greeting::Existing) {
            "access_tool_confirmation"
        } else {
            "access_tool_capability"
        };
        if let Some(binding) = &settings.access_tool {
            match &greeting {
                Greeting::Fresh(hello)
                    if hello.pointer("/agentCapabilities/mcpCapabilities/http")
                        == Some(&serde_json::Value::Bool(true)) => {}
                Greeting::Existing
                    if resume.as_deref().is_some_and(|id| {
                        binding.retained_for(id) || binding.prepared_for(id) || completed
                    }) => {}
                Greeting::Existing => return Err(unconfirmed_setup()),
                _ => {
                    return Err(AdapterError::Unsupported(
                        "runtime cannot establish the required service access request tool".into(),
                    ))
                }
            }
        }
        *phase = "authentication";
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

        timing.completed("protocol_access_setup");
        let defer_settings =
            matches!(greeting, Greeting::Existing) && settings.recovered_context_sequence.is_some();
        let session = match (greeting, resume) {
            (Greeting::Fresh(_), Some(conversation)) => {
                *phase = "session_load";
                self.activate_session(conversation.clone(), updates).await?;
                self.load_retained_session(serde_json::json!({
                    "sessionId": conversation,
                    "cwd": cwd,
                    "mcpServers": tool_servers,
                }))
                .await
                .map_err(|error| error.context("could not resume conversation"))?;
                conversation
            }
            (Greeting::Fresh(_), None) => {
                *phase = "session_new";
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
                *phase = "session_rejoin";
                if settings
                    .access_tool
                    .as_ref()
                    .is_some_and(|binding| binding.prepared_for(&conversation))
                {
                    if settings.own_credential
                        || settings.recovered_prompt.is_some()
                        || settings.channel_tool.is_some()
                        || settings.provider_tool.is_some()
                    {
                        return Err(AdapterError::Rejected(
                            "prepared conversation has incompatible owner work".into(),
                        ));
                    }
                    // The pinned provider keeps a live thread's old MCP config
                    // on load alone. Unsubscribe the idle prepared thread first,
                    // then load its same persisted identity with owner tools.
                    // A source-issued receipt is required before this branch;
                    // ordinary retained conversations never take it.
                    let mut rebinding = super::startup_timing::StartupTiming::new();
                    *phase = "prepared_session_close";
                    self.call(
                        "session/close",
                        serde_json::json!({ "sessionId": conversation }),
                    )
                    .await?;
                    rebinding.completed("prepared_protocol_close");
                    *phase = "prepared_session_load";
                    self.activate_session(conversation.clone(), updates).await?;
                    self.load_retained_session(serde_json::json!({
                        "sessionId": conversation,
                        "cwd": cwd,
                        "mcpServers": tool_servers,
                    }))
                    .await
                    .map_err(|error| error.context("could not bind prepared conversation"))?;
                    rebinding.completed("prepared_protocol_load");
                } else {
                    self.activate_session(conversation.clone(), updates).await?;
                }
                tracing::info!("rejoining the conversation held by the runtime");
                conversation
            }
            (Greeting::Existing, None) => {
                return Err(AdapterError::Rejected(
                    "an initialized bridge requires the held conversation identifier".into(),
                ));
            }
        };
        timing.completed("protocol_session");

        // A failed idle refresh can have closed its native session before load
        // completed. Applying settings here would reject forever before replay
        // can restore it. The restricted input accepts only the original refresh,
        // which loads this same ID and acknowledges all settings before publication.
        if let Some(mode) = settings.permission_mode.filter(|_| !defer_settings) {
            *phase = "permission_mode";
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
        if settings.channel_tool.is_some() && !defer_settings {
            // adapter_agent accepts this opaque binding only from the trusted
            // channel source after durable creation-provenance and exact-open
            // checks. Manual/Trigger Agents cannot opt in through runtime flags.
            // Apply on new/load/rejoin so an existing Agent also benefits, without
            // changing its immutable profile, home or launch request identity.
            self.configure_channel_effort(&session).await;
        }
        timing.completed("protocol_settings");
        if let Some((protocol, receipt)) = completion {
            *phase = "open_completion_recheck";
            self.recheck_open_completion(protocol, receipt).await?;
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

    async fn greet(
        &self,
        params: serde_json::Value,
        already_initialized_error: Option<&serde_json::Value>,
        resuming: bool,
    ) -> crate::Result<Greeting> {
        let idle_deadline = if resuming {
            RETAINED_HISTORY_IDLE_DEADLINE
        } else {
            CALL_DEADLINE
        };
        let frame = self
            .call_frame_within("initialize", params, idle_deadline)
            .await?;
        if already_initialized_error.is_some_and(|expected| frame.get("error") == Some(expected)) {
            tracing::info!("runtime bridge was already initialized");
            Ok(Greeting::Existing)
        } else {
            super::acp::rpc_result("initialize", frame).map(Greeting::Fresh)
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
fn protected_setup_failure(error: AdapterError, phase: &'static str) -> AdapterError {
    let detail = format!("channel tool session setup did not complete (phase: {phase})");
    match error {
        AdapterError::InputAuthorityUnavailable(_) => {
            AdapterError::InputAuthorityUnavailable(detail)
        }
        AdapterError::PromptNotWritten(_) => AdapterError::PromptNotWritten(detail),
        AdapterError::Unavailable(_) => AdapterError::Unavailable(detail),
        AdapterError::Rejected(_) | AdapterError::SkillUpgradeRefused(_) => {
            AdapterError::Rejected(detail)
        }
        AdapterError::OperationIdentityConflict(_) => {
            AdapterError::OperationIdentityConflict(detail)
        }
        AdapterError::RuntimeLedgerHold(_) => AdapterError::RuntimeLedgerHold(detail),
        AdapterError::AtCapacity(_) => AdapterError::AtCapacity(detail),
        AdapterError::Unsupported(_) => AdapterError::Unsupported(detail),
        AdapterError::RuntimeContractMismatch(_) => AdapterError::Rejected(detail),
    }
}

fn unconfirmed_setup() -> AdapterError {
    // An earlier initialize/open can have succeeded after its reply was lost.
    // Missing confirmation is not evidence that this runtime lacks an ability.
    // Without a known identity and a supported exact completion proof there is
    // no safe automatic recovery. Never create a substitute session or loop
    // indefinitely while the undispatched creation input remains queued.
    AdapterError::Rejected("existing conversation tool setup cannot be confirmed".into())
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
        || settings
            .recovered_context_sequence
            .is_some_and(|sequence| sequence == 0)
        || settings.recovered_context_sequence.is_some()
            && (settings.context_refresh.is_none() || settings.recovered_prompt.is_some())
    {
        return Err(AdapterError::Rejected(
            "ACP working directory or permission mode is invalid".into(),
        ));
    }
    Ok(())
}
