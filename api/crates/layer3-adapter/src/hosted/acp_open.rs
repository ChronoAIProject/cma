//! Open one attachment under its explicit transport flow-control policy.
use super::*;

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
            (settings, subscription, false),
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
        (settings, subscription, bounded): (
            HostedAgentOpenSettings,
            Option<crate::hosted::subscriptions::HostedSubscriptionBinding>,
            bool,
        ),
        resume_conversation: Option<String>,
        recovered_prompt: Option<PromptIdentity>,
        updates_tx: mpsc::Sender<ConversationUpdate>,
    ) -> crate::Result<OpenedBridge> {
        if settings.recovered_context_sequence.is_some()
            && (resume_conversation.is_none() || recovered_prompt.is_some())
        {
            return Err(AdapterError::Rejected(
                "context recovery requires one quiet retained conversation".into(),
            ));
        }
        if recovered_prompt.is_some() && resume_conversation.is_none() {
            return Err(AdapterError::Rejected(
                "prompt recovery requires a held conversation".into(),
            ));
        }
        // Quiet recovery already has an exact durable identity. Its receipts
        // must be available even before initialize replies: a failed load may
        // have left a full retained backlog on this same bridge.
        let startup_session = resume_conversation
            .clone()
            .filter(|_| recovered_prompt.is_none());
        let (sink, mut source) = stream.split();
        let declaration = tokio::time::timeout(DECLARATION_DEADLINE, source.next())
            .await
            .map_err(|_| AdapterError::Unavailable("runtime sent no Ready declaration".into()))?
            .ok_or_else(|| AdapterError::Unavailable("runtime closed before Ready".into()))?
            .map_err(|error| AdapterError::Unavailable(format!("Ready read failed: {error}")))?;
        let output_stream = validate_ready(declaration, expected_agent)?;
        if bounded && output_stream.is_none() {
            return Err(AdapterError::Unavailable(
                "bounded output requires retained runtime acknowledgements".into(),
            ));
        }
        let permission_state = if settings.bounded_permission_state {
            if output_stream.is_none() {
                return Err(AdapterError::Rejected(
                    "permission bounds require retained runtime output".into(),
                ));
            }
            let mut state = crate::hosted::permission_state::PermissionState::new()?;
            if let Some(prompt) = &recovered_prompt {
                state.begin_prompt(prompt.as_str());
            }
            Some(Mutex::new(state))
        } else {
            None
        };
        let (acknowledgements, acknowledged) = mpsc::channel(128);

        let bridge = Arc::new(Self {
            permission_state,
            subscription,
            sink: Arc::new(Mutex::new(sink)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            request_namespace: uuid::Uuid::new_v4().to_string(),
            next_id: AtomicI64::new(1),
            recovery: Mutex::new(recovery::PromptRecovery::new(recovered_prompt)),
            conversation: Mutex::new(ConversationState::default()),
            output: output_stream
                .map(|stream| Mutex::new(output::ReliableOutput::new(stream, acknowledgements))),
            blocked: Mutex::new(HashMap::new()),
            answered: Arc::new(Mutex::new(HashMap::new())),
            owned_prompt_waiters: Arc::new(AtomicUsize::new(0)),
            orphaned_prompt_response: AtomicBool::new(false),
            retired: AtomicBool::new(false),
            instructions_support: AtomicU8::new(0),
            context_refresh: settings.context_refresh.clone(),
            context_goal: Mutex::new(None),
            recovering_context: AtomicU64::new(settings.recovered_context_sequence.unwrap_or(0)),
        });
        let (updates, relay) = crate::hosted::update_relay::spawn(updates_tx, bounded);
        if let Some(session) = &startup_session {
            bridge.activate_session(session.clone(), &updates).await?;
            bridge.activate_output(session, &updates).await?;
        }
        let writer = Arc::clone(&bridge);
        let writer_updates = updates.clone();
        let input = ConversationInputSender::spawn(INPUT_BUFFER, move |input| {
            let writer = Arc::clone(&writer);
            let updates = writer_updates.clone();
            async move { writer.drive(input, &updates).await }
        })
        .with_settings_support(bridge.clone());

        let reader_task = reader::spawn(Arc::clone(&bridge), source, updates.clone(), acknowledged);
        let mut opening_tasks = OpeningTasks {
            bridge: Arc::clone(&bridge),
            reader: reader_task.abort_handle(),
            relay: relay.abort_handle(),
            armed: true,
        };

        bridge
            .handshake(resume_conversation, cwd, settings, &updates)
            .await?;
        if reader_task.is_finished() {
            return Err(AdapterError::Unavailable(
                "conversation link ended while opening".into(),
            ));
        }
        let conversation_id = bridge.session().await?;
        bridge.release_recovered_prompt(&updates).await;
        bridge.activate_output(&conversation_id, &updates).await?;
        opening_tasks.armed = false;
        Ok(OpenedBridge {
            channel: ConversationChannel {
                input,
                conversation_id: Some(conversation_id),
            },
            bridge,
            reader: reader_task,
            relay,
        })
    }
}
