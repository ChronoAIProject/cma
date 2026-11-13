//! Worker/store integration at the receipt boundary. The real Bridge's native
//! absence, active-goal, ambiguity and unavailable-proof cases live in the
//! adapter transport_refresh_tests; this fixture supplies their receipt result.
use super::*;
use layer3_adapter::{ConversationInput, ConversationSettingsSupport};

#[derive(Default)]
struct Reads {
    agent: AtomicU64,
    parent: AtomicU64,
}

struct ReattachedReadiness {
    receipt: Option<String>,
    reads: Arc<Reads>,
    receipt_calls: AtomicU64,
}

#[async_trait::async_trait]
impl ConversationSettingsSupport for ReattachedReadiness {
    async fn system_prompt_supported(&self) -> bool {
        false
    }

    async fn managed_context_may_be_ready(&self) -> bool {
        // A reattached Bridge with no replayed goal snapshot has this hint,
        // even when its separate transport absence proof can succeed.
        false
    }

    async fn managed_context_receipt(&self, _: u64) -> Option<String> {
        panic!("explicit migration must request the transport receipt")
    }

    async fn managed_transport_receipt(&self, _: u64) -> Option<String> {
        assert!(self.reads.agent.load(Ordering::SeqCst) > 0);
        assert!(self.reads.parent.load(Ordering::SeqCst) > 0);
        self.receipt_calls.fetch_add(1, Ordering::SeqCst);
        self.receipt.clone()
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn explicit_transport_reaches_fresh_admission_despite_unknown_local_goal() {
    for receipt in [Some("a".repeat(64)), None, Some("invalid".into())] {
        let valid = receipt.as_ref().is_some_and(|value| value.len() == 64);
        let name = format!("cma_transport_hint_{}", uuid::Uuid::new_v4().simple());
        let reads = Arc::new(Reads::default());
        let observed = reads.clone();
        let observed_database = name.clone();
        let mut options = ClientOptions::parse(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        options.command_event_handler = Some(EventHandler::callback(move |event| {
            if let CommandEvent::Started(event) = event {
                if event.db == observed_database
                    && event
                        .command
                        .get_document("filter")
                        .ok()
                        .is_some_and(|filter| filter.get_str("_id").is_ok())
                {
                    match event.command.get_str("find").ok() {
                        Some(AGENTS) => {
                            observed.agent.fetch_add(1, Ordering::SeqCst);
                        }
                        Some(SANDBOXES) => {
                            observed.parent.fetch_add(1, Ordering::SeqCst);
                        }
                        _ => (),
                    }
                }
            }
        }));
        let client = Client::with_options(options).unwrap();
        let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
        let mut agent = idle_connection_fixture(&store, "transport_hint", AgentStatus::Ready).await;
        let mut settings = agent.settings();
        settings.model = "model-a".into();
        agent.conversation.settings = Some(settings);
        agent.revision += 1;
        replace_agent_fixture(&store, &agent).await;
        let registry = Arc::new(LiveAgentRegistry::default());
        let proof = Arc::new(ReattachedReadiness {
            receipt,
            reads: reads.clone(),
            receipt_calls: AtomicU64::new(0),
        });
        let inputs = Arc::new(Mutex::new(Vec::new()));
        let captured = inputs.clone();
        let input = ConversationInputSender::spawn(1, move |input| {
            captured.lock().unwrap().push(input);
            async { Ok(()) }
        })
        .with_settings_support(proof.clone());
        let (_updates, receiver) = tokio::sync::mpsc::channel(1);
        let attachment = registry
            .attach(
                agent.id.clone(),
                agent.conversation.conversation_id.clone().unwrap(),
                agent.conversation.runtime_generation.unwrap(),
                agent.work.next_fence - 1,
                RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                input,
                receiver,
                Arc::new(IdleRecorder),
            )
            .unwrap();
        registry.publish_and_activate(&attachment).unwrap();
        let worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter: Arc::new(CodexAdapter::with_backend(
                    InMemoryHostedBackend::default(),
                    CodexSandboxSpec::default(),
                )),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: registry.clone(),
                updates: DurableAgentUpdateRecorder::new(store.clone()),
                operations: AgentOperationService::new(store.clone(), registry.clone()),
            },
            AgentWorkerConfig::new("transport-hint", "transport-hint")
                .with_managed_documents(true)
                .with_access_transport_migration(Some(crate::config::AgentAccessMigration {
                    revision: 2,
                    origin: "https://new.example".into(),
                    agents: vec![agent.id.to_string()],
                })),
        );
        reads.agent.store(0, Ordering::SeqCst);
        reads.parent.store(0, Ordering::SeqCst);
        assert_eq!(
            worker
                .reconcile_connections(SystemTime::now())
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            proof.receipt_calls.load(Ordering::SeqCst),
            1,
            "the ordinary negative hint must not suppress explicit proof"
        );
        let current = store.get_agent(&agent.id).await.unwrap().unwrap();
        if valid {
            assert_eq!(current.conversation.managed_guidance_revision, 2);
            assert_eq!(
                current.conversation.conversation_id,
                agent.conversation.conversation_id
            );
            assert_eq!(current.profile, agent.profile);
            assert_eq!(current.settings(), agent.settings());
            let inputs = inputs.lock().unwrap();
            assert_eq!(inputs.len(), 1);
            assert!(matches!(&inputs[0], ConversationInput::RefreshContext {
                service_origin: Some(origin), readiness_receipt, ..
            } if origin == "https://new.example" && readiness_receipt == &"a".repeat(64)));
        } else {
            assert_eq!(
                current, agent,
                "missing or invalid proof cannot mutate the Agent"
            );
            assert!(inputs.lock().unwrap().is_empty());
        }
        // Applied work is current; refused proof remains under the existing
        // optional-maintenance cooldown, not a retry on every scheduler tick.
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap();
        assert_eq!(proof.receipt_calls.load(Ordering::SeqCst), 1);
        client.database(&name).drop().await.unwrap();
    }
}
