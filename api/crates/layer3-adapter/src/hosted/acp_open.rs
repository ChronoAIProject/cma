//! Open one attachment under its explicit transport flow-control policy.
use super::*;
use crate::ingress_budget::diagnostic::make;
use crate::ingress_budget::{
    sizing, AdmittedError, AllocationPlan, IngressClass, IngressWorkScope, OwnedWork, SharedOwned,
};
use sha2::{Digest as _, Sha256};

type RelayPolicy<'a> = (bool, Option<&'a dyn crate::hosted::HostedOpenReplyLoss>);

impl Bridge {
    #[cfg(test)]
    pub(in crate::hosted) async fn open(
        stream: Socket,
        expected_agent: &str,
        cwd: String,
        settings: HostedAgentOpenSettings,
        resume_conversation: Option<String>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        Self::open_with_subscription(
            stream,
            expected_agent,
            cwd,
            (settings, None),
            resume_conversation,
            updates_tx,
        )
        .await
    }

    #[cfg(test)]
    pub(in crate::hosted) async fn open_with_subscription(
        stream: Socket,
        expected_agent: &str,
        cwd: String,
        (settings, subscription): (
            HostedAgentOpenSettings,
            Option<crate::hosted::subscriptions::HostedSubscriptionBinding>,
        ),
        resume_conversation: Option<String>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        Self::open_with_subscription_and_prompt(
            stream,
            expected_agent,
            cwd,
            (settings, subscription),
            resume_conversation,
            None,
            updates_tx,
        )
        .await
    }

    #[cfg(test)]
    pub(in crate::hosted) async fn open_with_subscription_and_prompt(
        stream: Socket,
        expected_agent: &str,
        cwd: String,
        (settings, subscription): (
            HostedAgentOpenSettings,
            Option<crate::hosted::subscriptions::HostedSubscriptionBinding>,
        ),
        resume_conversation: Option<String>,
        recovered_prompt: Option<PromptIdentity>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        Self::open_with_relay_policy(
            stream,
            expected_agent,
            cwd,
            (settings, subscription, (false, None)),
            resume_conversation,
            recovered_prompt,
            updates_tx,
        )
        .await
    }

    pub(in crate::hosted) async fn open_with_relay_policy(
        stream: Socket,
        expected_agent: &str,
        cwd: String,
        (settings, subscription, relay): (
            HostedAgentOpenSettings,
            Option<crate::hosted::subscriptions::HostedSubscriptionBinding>,
            RelayPolicy<'_>,
        ),
        resume_conversation: Option<String>,
        recovered_prompt: Option<PromptIdentity>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        Self::open_inner::<channel::Legacy>(
            ingress::legacy(stream),
            expected_agent,
            cwd,
            (settings, subscription.map(Into::into), relay, None),
            resume_conversation,
            recovered_prompt,
            updates_tx,
        )
        .await
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "admitted transport composition is supplied by the transport package"
        )
    )]
    pub(in crate::hosted) async fn open_admitted(
        stream: ingress::OwnedSocket,
        expected_agent: &str,
        cwd: String,
        (settings, subscription, attachment): (
            HostedAgentOpenSettings,
            Option<crate::hosted::subscriptions::AdmittedSubscriptionBinding>,
            crate::ingress_budget::IngressAttachment,
        ),
        resume_conversation: Option<String>,
        recovered_prompt: Option<PromptIdentity>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> Result<
        OwnedWork<
            OpenedBridge<crate::ingress_budget::AdmittedConversationChannel, SharedOwned<Bridge>>,
        >,
        AdmittedError,
    > {
        let policy = attachment.into_policy();
        ingress::components::require(&policy)?;
        if subscription
            .as_ref()
            .is_some_and(|binding| !binding.belongs_to(&policy))
        {
            return Err(crate::ingress_budget::AdmissionRefusal::InvalidPlan {
                phase: "subscription binding ownership",
            }
            .into());
        }
        if let Some(context) = &settings.context_refresh {
            context.require_owner(&policy)?;
        }
        let scope = IngressWorkScope::new(policy.admit(
            IngressClass::Data,
            AllocationPlan::checked("admitted open scope", &[sizing::shared_owner_bytes::<()>()])?,
        )?);
        scope
            .initialize_owned(|| async {
                if !(settings.retained_json
                    && settings.bounded_raw_output
                    && settings.bounded_permission_state
                    && settings.shared_runtime_output
                    && settings.output_json_budget)
                {
                    return Err(make(
                        AdapterError::Rejected,
                        format_args!("ingress ownership requires all decoder and delivery bounds"),
                    ));
                }
                Self::open_inner::<channel::Admitted>(
                    ingress::admitted(stream, policy.clone()),
                    expected_agent,
                    cwd,
                    (
                        settings,
                        subscription.map(Into::into),
                        (true, None),
                        Some(policy.clone()),
                    ),
                    resume_conversation,
                    recovered_prompt,
                    updates_tx,
                )
                .await
            })
            .resolve_admitted_result(&policy, IngressClass::Data)
            .await
    }

    async fn open_inner<M: channel::Mode>(
        stream: ingress::BridgeSocket,
        expected_agent: &str,
        cwd: String,
        (settings, subscription, (bounded, open_reply_loss), ingress): (
            HostedAgentOpenSettings,
            Option<crate::hosted::subscriptions::SubscriptionBinding>,
            RelayPolicy<'_>,
            Option<crate::ingress_budget::IngressOwnershipPolicy>,
        ),
        resume_conversation: Option<String>,
        recovered_prompt: Option<PromptIdentity>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge<M::Channel, M::Handle>> {
        if settings.coordinated_ingress
            && !(settings.retained_json
                && settings.bounded_raw_output
                && settings.bounded_permission_state)
        {
            return Err(make(
                AdapterError::Rejected,
                format_args!(
                    "ingress coordination requires JSON, raw-output and permission bounds"
                ),
            ));
        }
        let aggregate = ingress
            .as_ref()
            .map(|policy| policy.budget(crate::ingress_budget::IngressClass::Data))
            .or_else(|| {
                settings
                    .coordinated_ingress
                    .then(crate::ingress_budget::IngressBudget::process)
            });
        if settings.recovered_context_sequence.is_some()
            && (resume_conversation.is_none() || recovered_prompt.is_some())
        {
            return Err(make(
                AdapterError::Rejected,
                format_args!("context recovery requires one quiet retained conversation"),
            ));
        }
        if recovered_prompt.is_some() && resume_conversation.is_none() {
            return Err(make(
                AdapterError::Rejected,
                format_args!("prompt recovery requires a held conversation"),
            ));
        }
        // Quiet recovery already has an exact durable identity. Its receipts
        // must be available even before initialize replies: a failed load may
        // have left a full retained backlog on this same bridge.
        let _startup_identity = ingress
            .as_ref()
            .map(|policy| {
                policy.reserve(
                    crate::ingress_budget::IngressClass::Data,
                    crate::ingress_budget::AllocationPlan::new(
                        "startup conversation copy",
                        &[
                            resume_conversation.as_ref().map_or(0, String::len),
                            resume_conversation.as_ref().map_or(0, String::len),
                        ],
                    )?,
                )
            })
            .transpose()?;
        let startup_session = resume_conversation
            .clone()
            .filter(|_| recovered_prompt.is_none());
        let (sink, mut source) = stream.split()?;
        let declaration = tokio::time::timeout(DECLARATION_DEADLINE, source.next())
            .await
            .map_err(|_| {
                make(
                    AdapterError::Unavailable,
                    format_args!("runtime sent no Ready declaration"),
                )
            })?
            .ok_or_else(|| {
                make(
                    AdapterError::Unavailable,
                    format_args!("runtime closed before Ready"),
                )
            })?
            .map_err(|error| {
                make(
                    AdapterError::Unavailable,
                    format_args!("Ready read failed: {error}"),
                )
            })?;
        let output_json_budget = settings.output_json_budget;
        let json_pools = settings
            .retained_json
            .then(crate::hosted::retained_json::JsonPools::process)
            .map(|pools| match &ingress {
                Some(policy) => pools.with_ownership(policy),
                None => pools.with_aggregate(aggregate.clone()),
            });
        let ready_copy = match (&declaration, &ingress) {
            (ingress::IncomingFrame::Admitted(frame), Some(policy)) => {
                let Message::Text(text) = &**frame else {
                    return Err(make(
                        AdapterError::Rejected,
                        format_args!("runtime sent a non-text Ready declaration"),
                    ));
                };
                // validate_ready pre-admits parser scratch and the typed Ready
                // tree using JsonPools' node/string layout. Its independently
                // cloned input coexists with the admitted transport frame.
                Some(policy.reserve(
                    crate::ingress_budget::IngressClass::Transformation,
                    crate::ingress_budget::AllocationPlan::new("Ready input copy", &[text.len()])?,
                )?)
            }
            _ => None,
        };
        let output_stream = validate_ready(
            match declaration {
                ingress::IncomingFrame::Legacy(declaration) => declaration,
                ingress::IncomingFrame::Admitted(declaration) => (*declaration).clone(),
            },
            expected_agent,
            output_json_budget,
            json_pools.as_ref(),
        )?;
        drop(ready_copy);
        if json_pools.is_some() && output_stream.is_none() {
            return Err(make(
                AdapterError::Rejected,
                format_args!("JSON retention bounds require retained runtime output"),
            ));
        }
        if bounded && output_stream.is_none() {
            return Err(make(
                AdapterError::Unavailable,
                format_args!("bounded output requires retained runtime acknowledgements"),
            ));
        }
        if settings.bounded_raw_output && output_stream.is_none() {
            return Err(make(
                AdapterError::Rejected,
                format_args!("raw output bounds require retained runtime output"),
            ));
        }
        let permission_state = if settings.bounded_permission_state {
            if output_stream.is_none() {
                return Err(make(
                    AdapterError::Rejected,
                    format_args!("permission bounds require retained runtime output"),
                ));
            }
            let mut state =
                crate::hosted::permission_state::PermissionState::new(aggregate.clone())?;
            if let Some(prompt) = &recovered_prompt {
                state.begin_prompt(prompt.as_str());
            }
            Some(Mutex::new(state, ingress.as_ref())?)
        } else {
            None
        };
        let (acknowledgements, acknowledged) = match &ingress {
            Some(policy) => {
                let (sender, receiver) =
                    crate::RuntimeOutputReceipt::admitted_channel(policy.clone(), 128)?;
                (
                    ingress::AckSender::Admitted(sender),
                    ingress::AckReceiver::Admitted(receiver),
                )
            }
            None => {
                let (sender, receiver) = mpsc::channel(128);
                (
                    ingress::AckSender::Legacy(sender),
                    ingress::AckReceiver::Legacy(receiver),
                )
            }
        };

        let identity_storage = ingress::identity::bridge_identity_storage(ingress.as_ref())?;
        let bridge = M::own(
            Self {
                ingress: ingress.clone(),
                authority: crate::owner_protocol::current_authority(),
                json_pools: json_pools.clone(),
                bounded_raw_output: settings.bounded_raw_output,
                shared_runtime_output: settings.shared_runtime_output,
                output_json_budget,
                permission_state,
                agent_correlation_digest: format!(
                    "{:x}",
                    Sha256::digest(expected_agent.as_bytes())
                ),
                subscription,
                sink: Shared::new(
                    Mutex::new(sink, ingress.as_ref())?,
                    ingress.as_ref(),
                    "shared bridge sink",
                )?,
                pending: Shared::new(
                    Mutex::new(
                        ingress::identity::PendingCalls::new(ingress.as_ref())?,
                        ingress.as_ref(),
                    )?,
                    ingress.as_ref(),
                    "shared pending calls",
                )?,
                request_namespace: uuid::Uuid::new_v4().to_string(),
                next_id: AtomicI64::new(1),
                recovery: Mutex::new(
                    recovery::PromptRecovery::new(recovered_prompt, ingress.as_ref())?,
                    ingress.as_ref(),
                )?,
                conversation: Mutex::new(ConversationState::default(), ingress.as_ref())?,
                output: output_stream
                    .map(|stream| {
                        output::ReliableOutput::new(
                            stream,
                            acknowledgements,
                            settings.bounded_raw_output,
                            json_pools,
                            aggregate,
                            ingress.as_ref(),
                        )
                        .and_then(|value| Mutex::new(value, ingress.as_ref()).map_err(Into::into))
                    })
                    .transpose()?,
                blocked: Mutex::new(HashMap::new(), ingress.as_ref())?,
                answered: Shared::new(
                    Mutex::new(HashMap::new(), ingress.as_ref())?,
                    ingress.as_ref(),
                    "shared permission answers",
                )?,
                owned_prompt_waiters: Shared::new(
                    AtomicUsize::new(0),
                    ingress.as_ref(),
                    "shared prompt waiters",
                )?,
                orphaned_prompt_response: AtomicBool::new(false),
                retired: AtomicBool::new(false),
                instructions_support: AtomicU8::new(0),
                context_refresh: settings.context_refresh.clone(),
                context_goal: Mutex::new(None, ingress.as_ref())?,
                retained_history_deadline: progress::ProgressSender::new(None, ingress.as_ref())?,
                recovering_context: AtomicU64::new(
                    settings.recovered_context_sequence.unwrap_or(0),
                ),
                _identity_storage: identity_storage,
            },
            ingress.as_ref(),
        )?;
        let (updates, relay) = match &ingress {
            Some(policy) => crate::hosted::update_relay::spawn_admitted(updates_tx, policy)?,
            None => crate::hosted::update_relay::spawn(updates_tx, bounded),
        };
        if let Some(session) = &startup_session {
            bridge.activate_session(session.clone(), &updates).await?;
            bridge.activate_output(session, &updates).await?;
        }
        let input = M::input(&bridge, &updates).await?;

        let reader_task = reader::spawn(bridge.clone(), source, updates.clone(), acknowledged);
        let mut opening_tasks = OpeningTasks {
            bridge: bridge.clone(),
            reader: reader_task.abort_handle(),
            relay: relay.abort_handle(),
            armed: true,
        };

        bridge
            .handshake(
                resume_conversation,
                cwd,
                settings,
                &updates,
                open_reply_loss,
            )
            .await?;
        if reader_task.is_finished() {
            return Err(make(
                AdapterError::Unavailable,
                format_args!("conversation link ended while opening"),
            ));
        }
        let public_identity = ingress
            .as_ref()
            .map(|policy| {
                policy.reserve(
                    IngressClass::Data,
                    AllocationPlan::new(
                        "published conversation identity",
                        &[
                            super::super::acp_session_identity::MAX_BYTES,
                            sizing::shared_owner_bytes::<String>(),
                        ],
                    )?,
                )
            })
            .transpose()?;
        let conversation_id = bridge.session().await?;
        bridge.release_recovered_prompt(&updates).await;
        bridge.activate_output(&conversation_id, &updates).await?;
        let channel = M::finish(input, conversation_id, public_identity)?;
        opening_tasks.armed = false;
        Ok(OpenedBridge {
            channel,
            bridge,
            reader: reader_task,
            relay,
        })
    }
}
