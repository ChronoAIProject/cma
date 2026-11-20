use super::*;
use crate::resource_execution_protocol::*;
use crate::{ContentBlock, PermissionDecision};
use std::sync::atomic::AtomicUsize;
use tokio::sync::Notify;

fn input(text: &str) -> ResourceInput {
    ResourceInput::admit(ConversationInput::Prompt(vec![ContentBlock::Text(
        agent_client_protocol_schema::v1::TextContent::new(text),
    )]))
    .unwrap()
}

fn ticket(input: &ResourceInput) -> ExecutionTicketV1 {
    let id = |value| OpaqueId::new(value).unwrap();
    let digest = |value: char| Digest::new(&value.to_string().repeat(64)).unwrap();
    let generation = |value| Generation::new(value).unwrap();
    let receiver = ReceiverKey {
        kind: ReceiverKind::Runtime,
        instance: id("runtime"),
        incarnation: digest('a'),
        boot: id("boot"),
    };
    ExecutionTicketV1 {
        protocol: 1,
        execution: ExecutionKey {
            command_id: id("command"),
            command_fingerprint: digest('b'),
            dispatch_sequence: generation(1),
            prompt_id: id("prompt"),
            turn_id: id("turn"),
            agent_id: id("agent"),
            receiver: receiver.clone(),
            attempt: generation(1),
        },
        basis: ExecutionBasisV1::Share {
            grant: GrantKey {
                family: GrantFamilyKey {
                    origin: digest('c'),
                    share_id: id("share"),
                    generation: generation(1),
                },
                share_revision: generation(1),
                target_ancestry: digest('d'),
                capability: input.operation().capability(),
                policy_revision: generation(1),
                actor: digest('e'),
            },
        },
        operation: input.operation(),
        input_digest: input.digest().clone(),
        manager: ManagerKey {
            instance: id("manager"),
            epoch: generation(1),
            agent_id: id("agent"),
        },
        challenge: ReceiverChallengeV1 {
            protocol: 1,
            receiver,
            nonce: id("challenge"),
            created_tick_ms: 1,
            deadline_tick_ms: 3001,
        },
        nonce: id("ticket"),
        registration: digest('f'),
    }
}

fn dispatch() -> ResourceDispatch {
    let input = input("synthetic prompt");
    let ticket = ticket(&input);
    input.bind(ticket).unwrap()
}

struct Guard {
    accepted: bool,
    checked: AtomicUsize,
}
#[async_trait::async_trait]
impl ConversationInputGuard for Guard {
    async fn check(&self) -> crate::Result<()> {
        self.checked.fetch_add(1, Ordering::SeqCst);
        if self.accepted {
            Ok(())
        } else {
            Err(AdapterError::Rejected("guard refused".into()))
        }
    }
}
fn guard(accepted: bool) -> Arc<Guard> {
    Arc::new(Guard {
        accepted,
        checked: AtomicUsize::new(0),
    })
}

struct Transport {
    calls: AtomicUsize,
    corrupt: bool,
    hang: bool,
    entered: Notify,
}
impl RuntimeExecutionControl for Transport {
    fn capabilities(&self) -> Option<ExecutionCapabilities> {
        Some(ExecutionCapabilities {
            protocol: 1,
            targeted_native_stop: true,
            mediated_effects: true,
            mixed_principal_control: false,
        })
    }
}
#[async_trait::async_trait]
impl ResourceConversationTransport for Transport {
    async fn send_resource(&self, dispatch: &ResourceDispatch) -> ResourceSendOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        if self.hang {
            std::future::pending::<()>().await;
        }
        let ticket = dispatch.ticket();
        ResourceSendOutcome::Observed(Box::new(StartObservationV1 {
            protocol: 1,
            execution: ticket.execution.clone(),
            registration: ticket.registration.clone(),
            nonce: if self.corrupt {
                OpaqueId::new("another-ticket").unwrap()
            } else {
                ticket.nonce.clone()
            },
            state: NativeStartState::Entered,
            sequence: Generation::new(2).unwrap(),
        }))
    }
}
fn transport(corrupt: bool, hang: bool) -> Arc<Transport> {
    Arc::new(Transport {
        calls: AtomicUsize::new(0),
        corrupt,
        hang,
        entered: Notify::new(),
    })
}

#[test]
fn input_digest_binds_semantics_and_canonical_encoding() {
    let first = input("first");
    let second = input("second");
    assert_ne!(first.digest(), second.digest());
    let decoded = ResourceInput::decode(first.bytes()).unwrap();
    assert_eq!(decoded.digest(), first.digest());
    assert_eq!(decoded.bytes(), first.bytes());
    assert!(matches!(
        second.bind(ticket(&first)),
        Err(ProtocolError::EvidenceMismatch)
    ));
    let mut changed = ticket(&first);
    changed.operation = ExecutionOperation::Steer;
    assert!(first.bind(changed).is_err());
}

#[test]
fn input_refuses_legacy_frames_unknown_fields_duplicates_and_noncanonical_bytes() {
    for raw in [
        r#"{"method":"session/prompt","params":{}}"#,
        r#"{"input":{"operation":"prompt","content":[],"extra":true},"protocol":1}"#,
        r#"{"input":{"operation":"prompt","content":[],"content":[]},"protocol":1}"#,
        r#"{"input":{"operation":"prompt","content":[]},"protocol":1,"protocol":1}"#,
        r#"{"protocol":1,"input":{"operation":"prompt","content":[]}}"#,
    ] {
        assert!(
            ResourceInput::decode(raw.as_bytes()).is_err(),
            "accepted {raw}"
        );
    }
    assert!(matches!(
        ResourceInput::admit(ConversationInput::Cancel),
        Err(ProtocolError::Unsupported)
    ));
    assert!(
        ResourceInput::admit(ConversationInput::Prompt(vec![ContentBlock::Text(
            agent_client_protocol_schema::v1::TextContent::new("x".repeat(MAX_MESSAGE_BYTES)),
        )]))
        .is_err()
    );
}

#[test]
fn settings_context_permission_and_recovery_fields_are_bound() {
    let configure = |system_prompt| ConversationInput::Configure {
        model: "synthetic".into(),
        reasoning_effort: crate::ReasoningEffort::High,
        permission: crate::PermissionLevel::Autonomous,
        system_prompt,
    };
    assert_ne!(
        ResourceInput::admit(configure(None)).unwrap().digest(),
        ResourceInput::admit(configure(Some("guidance".into())))
            .unwrap()
            .digest()
    );
    let permission = |decision| ConversationInput::PermissionAnswer {
        id: "approval".into(),
        locator: None,
        decision,
    };
    assert_ne!(
        ResourceInput::admit(permission(PermissionDecision::Allow))
            .unwrap()
            .digest(),
        ResourceInput::admit(permission(PermissionDecision::Deny))
            .unwrap()
            .digest()
    );
    let context = |operation_sequence| ConversationInput::RefreshContext {
        service_origin: Some("https://example.test".into()),
        operation_sequence,
        readiness_receipt: "readiness".into(),
        previous_guidance: vec!["old".into()],
        working_directory: Some("/workspace".into()),
        guidance: "new".into(),
        model: "synthetic".into(),
        reasoning_effort: crate::ReasoningEffort::High,
        permission: crate::PermissionLevel::Autonomous,
    };
    let a = ResourceInput::admit(context(1)).unwrap();
    let b = ResourceInput::admit(context(2)).unwrap();
    assert_eq!(a.operation(), ExecutionOperation::ContextChange);
    assert_ne!(a.digest(), b.digest());
    let recovery = |sequence| ConversationInput::RecoverablePrompt {
        content: vec![],
        prompt_id: crate::PromptIdentity::new("conversation", 1, sequence),
    };
    assert_ne!(
        ResourceInput::admit(recovery(1)).unwrap().digest(),
        ResourceInput::admit(recovery(2)).unwrap().digest()
    );
    assert!(!format!("{a:?}").contains("example.test"));
}

#[tokio::test]
async fn missing_resource_handler_refuses_without_legacy_fallback_or_retiring_writer() {
    let legacy_calls = Arc::new(AtomicUsize::new(0));
    let seen = legacy_calls.clone();
    let sender = ConversationInputSender::spawn(4, move |_| {
        seen.fetch_add(1, Ordering::SeqCst);
        async { Ok(()) }
    });
    assert!(matches!(
        sender.send_resource_guarded(dispatch(), guard(true)).await,
        ResourceSendOutcome::Refused(AdapterError::Unsupported(_))
    ));
    assert_eq!(legacy_calls.load(Ordering::SeqCst), 0);
    sender.send(ConversationInput::Cancel).await.unwrap();
    assert_eq!(legacy_calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn original_guard_runs_before_resource_handler_and_refusal_keeps_channel() {
    let handler = transport(false, false);
    let sender = ConversationInputSender::spawn(4, |_| async { Ok(()) })
        .with_resource_transport(handler.clone())
        .unwrap();
    let refused = guard(false);
    assert!(matches!(
        sender
            .send_resource_guarded(dispatch(), refused.clone())
            .await,
        ResourceSendOutcome::Refused(AdapterError::Rejected(_))
    ));
    assert_eq!(refused.checked.load(Ordering::SeqCst), 1);
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        sender.send_resource_guarded(dispatch(), guard(true)).await,
        ResourceSendOutcome::Observed(_)
    ));
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn resource_input_cannot_overtake_legacy_input_or_skip_queue_guard() {
    let legacy_entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let entered = legacy_entered.clone();
    let resume = release.clone();
    let sender = ConversationInputSender::spawn(4, move |_| {
        let entered = entered.clone();
        let resume = resume.clone();
        async move {
            entered.notify_one();
            resume.notified().await;
            Ok(())
        }
    });
    let handler = transport(false, false);
    let sender = sender.with_resource_transport(handler.clone()).unwrap();
    let legacy_sender = sender.clone();
    let legacy = tokio::spawn(async move { legacy_sender.send(ConversationInput::Cancel).await });
    legacy_entered.notified().await;
    let checked = guard(true);
    let resource_sender = sender.clone();
    let resource_guard = checked.clone();
    let resource = tokio::spawn(async move {
        resource_sender
            .send_resource_guarded(dispatch(), resource_guard)
            .await
    });
    while sender.inner.capacity() == 4 {
        tokio::task::yield_now().await;
    }
    assert_eq!(checked.checked.load(Ordering::SeqCst), 0);
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
    release.notify_one();
    legacy.await.unwrap().unwrap();
    assert!(matches!(
        resource.await.unwrap(),
        ResourceSendOutcome::Observed(_)
    ));
    assert_eq!(checked.checked.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn mismatched_native_evidence_is_uncertain_and_retires_the_writer() {
    let sender = ConversationInputSender::spawn(4, |_| async { Ok(()) })
        .with_resource_transport(transport(true, false))
        .unwrap();
    assert!(matches!(
        sender.send_resource_guarded(dispatch(), guard(true)).await,
        ResourceSendOutcome::Uncertain(_)
    ));
    assert!(sender.send(ConversationInput::Cancel).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn resource_timeout_after_handler_entry_is_uncertain_and_cannot_retry() {
    let handler = transport(false, true);
    let sender = ConversationInputSender::spawn(4, |_| async { Ok(()) })
        .with_resource_transport(handler.clone())
        .unwrap();
    let sending = sender.clone();
    let request =
        tokio::spawn(async move { sending.send_resource_guarded(dispatch(), guard(true)).await });
    handler.entered.notified().await;
    tokio::time::advance(Duration::from_secs(3)).await;
    assert!(matches!(
        request.await.unwrap(),
        ResourceSendOutcome::Uncertain(_)
    ));
    assert!(matches!(
        sender.send_resource_guarded(dispatch(), guard(true)).await,
        ResourceSendOutcome::Refused(_)
    ));
    assert_eq!(handler.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn cancelled_resource_input_waiting_for_guard_never_reaches_the_handler() {
    struct WaitingGuard(Notify);
    #[async_trait::async_trait]
    impl ConversationInputGuard for WaitingGuard {
        async fn check(&self) -> crate::Result<()> {
            self.0.notify_one();
            std::future::pending().await
        }
    }
    let handler = transport(false, false);
    let sender = ConversationInputSender::spawn(4, |_| async { Ok(()) })
        .with_resource_transport(handler.clone())
        .unwrap();
    let waiting = Arc::new(WaitingGuard(Notify::new()));
    let sending = sender.clone();
    let checking = waiting.clone();
    let request =
        tokio::spawn(async move { sending.send_resource_guarded(dispatch(), checking).await });
    waiting.0.notified().await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    sender.send(ConversationInput::Cancel).await.unwrap();
    assert_eq!(handler.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn recovered_prompt_must_match_the_ticket_prompt_identity() {
    let input = ResourceInput::admit(ConversationInput::RecoverablePrompt {
        content: vec![],
        prompt_id: crate::PromptIdentity::new("conversation", 1, 1),
    })
    .unwrap();
    assert!(matches!(
        input.clone().bind(ticket(&input)),
        Err(ProtocolError::EvidenceMismatch)
    ));
    let mut matching = ticket(&input);
    matching.execution.prompt_id =
        OpaqueId::new(crate::PromptIdentity::new("conversation", 1, 1).as_str()).unwrap();
    assert!(input.bind(matching).is_ok());
}

#[tokio::test]
async fn absent_or_partial_capabilities_cannot_bind_a_resource_handler() {
    struct CapabilityOnly(Option<ExecutionCapabilities>);
    impl RuntimeExecutionControl for CapabilityOnly {
        fn capabilities(&self) -> Option<ExecutionCapabilities> {
            self.0
        }
    }
    #[async_trait::async_trait]
    impl ResourceConversationTransport for CapabilityOnly {
        async fn send_resource(&self, _: &ResourceDispatch) -> ResourceSendOutcome {
            panic!("unqualified transport must never be entered")
        }
    }
    for capability in [
        None,
        Some(ExecutionCapabilities {
            protocol: 2,
            targeted_native_stop: true,
            mediated_effects: true,
            mixed_principal_control: false,
        }),
        Some(ExecutionCapabilities {
            protocol: 1,
            targeted_native_stop: false,
            mediated_effects: true,
            mixed_principal_control: false,
        }),
        Some(ExecutionCapabilities {
            protocol: 1,
            targeted_native_stop: true,
            mediated_effects: false,
            mixed_principal_control: false,
        }),
    ] {
        let sender = ConversationInputSender::spawn(1, |_| async { Ok(()) });
        let legacy = sender.clone();
        assert!(matches!(
            sender.with_resource_transport(Arc::new(CapabilityOnly(capability))),
            Err(AdapterError::Unsupported(_))
        ));
        legacy.send(ConversationInput::Cancel).await.unwrap();
    }
}
