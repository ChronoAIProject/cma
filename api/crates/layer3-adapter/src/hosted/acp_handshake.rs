use super::acp::{
    ingress::identity, Bridge, CALL_DEADLINE, RETAINED_HISTORY_IDLE_DEADLINE, SIGN_IN_DEADLINE,
};
use super::update_relay::UpdateRelay;
use super::{HostedAgentOpenSettings, HostedOpenReplyLoss};
use crate::ingress_budget::diagnostic::text;
use crate::AdapterError;

#[path = "acp_handshake_errors.rs"]
mod errors;
use errors::{protected_setup_failure, unconfirmed_setup, validate_settings};

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
        open_reply_loss: Option<&dyn HostedOpenReplyLoss>,
    ) -> crate::Result<()> {
        let protected = settings.channel_tool.is_some()
            || settings.provider_tool.is_some()
            || settings.access_tool.is_some();
        let mut phase = "settings_validation";
        self.handshake_inner(resume, cwd, settings, updates, open_reply_loss, &mut phase)
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
        open_reply_loss: Option<&dyn HostedOpenReplyLoss>,
        phase: &mut &'static str,
    ) -> crate::Result<()> {
        validate_settings(&cwd, &settings)?;
        let _preparation_storage = self
            .ingress
            .as_ref()
            .map(|policy| {
                policy.reserve(
                    crate::ingress_budget::IngressClass::Data,
                    identity::handshake_plan(&cwd, &settings, self.agent_correlation_digest.len())?,
                )
            })
            .transpose()?;
        let mut timing =
            super::startup_timing::StartupTiming::for_agent(self.agent_correlation_digest.clone());
        *phase = "protocol_initialize";
        let greeting = self
            .greet(
                identity::initialization_params(),
                settings.already_initialized_error.as_ref(),
                resume.is_some(),
            )
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
        // An earlier attempt may have initialized this exact bridge and opened
        // the conversation before it was interrupted, leaving no identifier to
        // rejoin. Adopt only the single completed open the runtime proves for
        // this Agent's binding; its exact receipt is verified again below.
        let resume = match (&greeting, resume, settings.open_completion) {
            (Greeting::Existing, None, Some(protocol)) => {
                *phase = "open_adoption";
                let digest = super::acp_open_completion::binding_digest_with_policy(
                    &cwd,
                    &tool_servers,
                    self.ingress.as_ref(),
                )?;
                match self.current_open_completion(protocol, &digest).await? {
                    super::acp_open_completion::CurrentOpen::Adopt(conversation) => {
                        tracing::info!("adopting the completed open of an interrupted attempt");
                        Some(conversation)
                    }
                    super::acp_open_completion::CurrentOpen::InFlight => {
                        return Err(AdapterError::Unavailable(text(
                            "an earlier open of this conversation is still completing",
                        )))
                    }
                    super::acp_open_completion::CurrentOpen::Unproven => None,
                }
            }
            (_, resume, _) => resume,
        };
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
                let digest = super::acp_open_completion::binding_digest_with_policy(
                    &cwd,
                    &tool_servers,
                    self.ingress.as_ref(),
                )?;
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
                    return Err(AdapterError::Unsupported(text(
                        "runtime cannot establish the required channel reply tool connection",
                    )))
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
                    return Err(AdapterError::Unsupported(text(
                        "runtime cannot establish the required provider tools",
                    )))
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
                    return Err(AdapterError::Unsupported(text(
                        "runtime cannot establish the required service access request tool",
                    )))
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
                timing
                    .subphase(
                        "fresh_load",
                        "activate_session",
                        self.activate_session(conversation.clone(), updates),
                    )
                    .await?;
                timing
                    .subphase(
                        "fresh_load",
                        "session_load",
                        self.load_retained_session(serde_json::json!({
                            "sessionId": conversation,
                            "cwd": cwd,
                            "mcpServers": tool_servers,
                        })),
                    )
                    .await
                    .map_err(|error| error.context("could not resume conversation"))?;
                conversation
            }
            (Greeting::Fresh(_), None) => {
                *phase = "session_new";
                let started = timing
                    .subphase(
                        "fresh_new",
                        "session_new",
                        self.call(
                            "session/new",
                            serde_json::json!({ "cwd": cwd, "mcpServers": tool_servers }),
                        ),
                    )
                    .await?;
                let conversation =
                    super::acp_session_identity::read(&started, self.json_pools.is_some())?;
                if let (Some(claim), Some(protocol)) = (open_reply_loss, settings.open_completion) {
                    self.drop_proven_open_reply(
                        claim,
                        protocol,
                        &conversation,
                        &cwd,
                        &tool_servers,
                    )
                    .await?;
                }
                timing
                    .subphase(
                        "fresh_new",
                        "activate_session",
                        self.activate_session(conversation.clone(), updates),
                    )
                    .await?;
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
                        return Err(AdapterError::Rejected(text(
                            "prepared conversation has incompatible owner work",
                        )));
                    }
                    // The pinned provider keeps a live thread's old MCP config
                    // on load alone. Unsubscribe the idle prepared thread first,
                    // then load its same persisted identity with owner tools.
                    // A source-issued receipt is required before this branch;
                    // ordinary retained conversations never take it.
                    let mut rebinding = super::startup_timing::StartupTiming::new();
                    *phase = "prepared_session_close";
                    timing
                        .subphase(
                            "prepared_rejoin",
                            "session_close",
                            self.call(
                                "session/close",
                                serde_json::json!({ "sessionId": conversation }),
                            ),
                        )
                        .await?;
                    rebinding.completed("prepared_protocol_close");
                    *phase = "prepared_session_load";
                    timing
                        .subphase(
                            "prepared_rejoin",
                            "activate_session",
                            self.activate_session(conversation.clone(), updates),
                        )
                        .await?;
                    timing
                        .subphase(
                            "prepared_rejoin",
                            "session_load",
                            self.load_retained_session(serde_json::json!({
                                "sessionId": conversation,
                                "cwd": cwd,
                                "mcpServers": tool_servers,
                            })),
                        )
                        .await
                        .map_err(|error| error.context("could not bind prepared conversation"))?;
                    rebinding.completed("prepared_protocol_load");
                } else {
                    timing
                        .subphase(
                            "existing_rejoin",
                            "activate_session",
                            self.activate_session(conversation.clone(), updates),
                        )
                        .await?;
                }
                tracing::info!("rejoining the conversation held by the runtime");
                conversation
            }
            (Greeting::Existing, None) => {
                return Err(AdapterError::Rejected(text(
                    "an initialized bridge requires the held conversation identifier",
                )));
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
                error.context_formatted(format_args!(
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
                AdapterError::Unsupported(text(
                    "runtime offers no remotely completable owner sign-in",
                ))
            })?;
        let _method_storage = self
            .ingress
            .as_ref()
            .map(|policy| {
                policy.reserve(
                    crate::ingress_budget::IngressClass::Data,
                    crate::ingress_budget::AllocationPlan::new(
                        "authentication method JSON",
                        &[
                            chosen.len(),
                            11 * size_of::<(String, serde_json::Value)>() + 14 * size_of::<usize>(),
                            "methodId".len(),
                        ],
                    )?,
                )
            })
            .transpose()?;
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
