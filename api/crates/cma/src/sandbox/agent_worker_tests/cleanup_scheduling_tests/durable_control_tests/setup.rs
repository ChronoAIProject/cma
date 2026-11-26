use super::*;

impl Fixture {
    pub(super) async fn new(deleting: bool) -> Self {
        Self::with_modes(deleting, false, false, Some("owner")).await
    }

    pub(super) async fn with_modes(
        deleting: bool,
        segmented: bool,
        effect_fence: bool,
        identity: Option<&str>,
    ) -> Self {
        Self::with_test_modes(
            deleting,
            segmented,
            effect_fence,
            identity,
            #[cfg(feature = "qualification-hooks")]
            None,
        )
        .await
    }

    #[cfg(feature = "qualification-hooks")]
    pub(super) async fn with_checkpoints(
        deleting: bool,
        checkpoints: [Arc<cma_qualification::Registry>; 2],
    ) -> Self {
        Self::with_test_modes(deleting, false, false, Some("owner"), Some(checkpoints)).await
    }

    async fn with_test_modes(
        deleting: bool,
        segmented: bool,
        effect_fence: bool,
        identity: Option<&str>,
        #[cfg(feature = "qualification-hooks")] checkpoints: Option<
            [Arc<cma_qualification::Registry>; 2],
        >,
    ) -> Self {
        let uri = std::env::var("CHRONO_TEST_MONGODB_FAILPOINT_URI").unwrap();
        let name = format!("cma_durable_control_{}", uuid::Uuid::new_v4().simple());
        let app_name = format!("control_{name}");
        let usage = Arc::new(Mutex::new(measurements::Usage::default()));
        let failed_commands = Arc::new(AtomicUsize::new(0));
        let failures = failed_commands.clone();
        let ownership_io = Arc::new(AtomicUsize::new(0));
        let control_ownership = ownership_io.clone();
        let commands = Arc::new(Mutex::new(Vec::new()));
        let trace = commands.clone();
        let mut options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
        assert!(
            options.hosts.iter().all(|address| matches!(address,
                mongodb::options::ServerAddress::Tcp { host, .. }
                    if matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1")
            )),
            "disposable loopback replica set required"
        );
        options.max_pool_size = Some(30);
        options.cmap_event_handler = Some(measurements::observe(usage.clone()));
        options.app_name = Some(app_name.clone());
        options.command_event_handler =
            Some(mongodb::event::EventHandler::callback(move |event| {
                measurements::note_ownership(&event, &control_ownership);
                match event {
                    mongodb::event::command::CommandEvent::Failed(_) => {
                        failures.fetch_add(1, Ordering::SeqCst);
                    }
                    mongodb::event::command::CommandEvent::Started(event) => {
                        trace.lock().unwrap().push(event.command_name);
                    }
                    _ => {}
                }
            }));
        let control_client = Client::with_options(options).unwrap();
        let mut admin_options = mongodb::options::ClientOptions::parse(&uri).await.unwrap();
        let observed_ownership = ownership_io.clone();
        admin_options.command_event_handler =
            Some(mongodb::event::EventHandler::callback(move |event| {
                measurements::note_ownership(&event, &observed_ownership);
            }));
        let admin = Client::with_options(admin_options).unwrap();
        let store = Arc::new(
            MongoSandboxStore::open(&control_client, &name)
                .await
                .unwrap(),
        );
        let h = Harness::with_store(store.clone(), Arc::new(EmptyAgentSkillResolver), 1).await;
        let agent = h.admit("repro", vec![]).await;
        let mut agent = h.drive_ready(&agent.id).await;
        if deleting {
            store
                .schedule_agent_stop(layer2_core::sandbox::store::AgentStopRequest {
                    target: AgentControlTarget::from(&agent),
                    reason: AgentStopReason::Owner,
                    requested_at: layer2_core::now_rfc3339(),
                })
                .await
                .unwrap();
            for _ in 0..5 {
                h.worker
                    .run_one(&agent.id, SystemTime::now())
                    .await
                    .unwrap();
            }
            agent = h.stored(&agent.id).await;
            assert_eq!(agent.status, AgentStatus::Stopped);
        }
        if segmented {
            store.install_segmented_control_fixture(&agent).await;
        }
        let ownership = Arc::new(MongoAgentOwnership::open(&admin, &name).await.unwrap());
        let operations = Arc::new(
            MongoOperationStore::open_for_test(&admin, &name)
                .await
                .unwrap(),
        );
        let cancel_after_fence = Arc::new(AtomicBool::new(false));
        let mut coordinators = vec![];
        let mut listeners = vec![];
        for i in 0..2 {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = format!("http://{}", listener.local_addr().unwrap());
            coordinators.push(
                OwnershipCoordinator::register(
                    ownership.clone(),
                    Member {
                        instance_id: format!("am-{i}"),
                        protocol: PROTOCOL_VERSION,
                        relay_address: address,
                        expires_at_ms: 0,
                        draining: false,
                        owned_agents: 0,
                        max_owned_agents: 256,
                    },
                )
                .await
                .unwrap(),
            );
            listeners.push(listener);
        }
        let selected = candidates(&agent.id, &ownership.membership().await.unwrap(), None).unwrap()
            [0]
        .instance_id
        .clone();
        let owner = coordinators
            .iter()
            .position(|c| c.instance_id == selected)
            .unwrap();
        let authority = coordinators[owner]
            .acquire(&agent.id)
            .await
            .unwrap()
            .unwrap();
        let cancel_after_admission = Arc::new(AtomicBool::new(false));
        let operations = Arc::new(idempotency::CancelAfterAdmission {
            inner: operations,
            authority: authority.clone(),
            cancel: cancel_after_admission.clone(),
        });
        let mut replicas = vec![];
        for (i, (coordinator, listener)) in coordinators.into_iter().zip(listeners).enumerate() {
            let live = if i == owner {
                h.registry.clone()
            } else {
                Arc::new(LiveAgentRegistry::default())
            };
            let registry = Arc::new(Registry {
                cancel_authority: authority.clone(),
                live: live.clone(),
                cancel_after_fence: cancel_after_fence.clone(),
            });
            let store = if effect_fence {
                Arc::new(
                    MongoSandboxStore::open(&control_client, &name)
                        .await
                        .unwrap()
                        .enable_agent_ownership(format!("am-{i}"))
                        .await
                        .unwrap(),
                )
            } else {
                store.clone()
            };
            let updates = DurableAgentUpdateRecorder::new(store.clone());
            let agents = AgentService::new(
                store.clone(),
                Arc::new(layer2_core::profiles::store::InMemoryProfileStore::default()),
                registry.clone(),
                Default::default(),
            );
            let reads = AgentReadService::new(store.clone(), Arc::new(Resources));
            let ops = AgentOperationService::new(store.clone(), registry);
            let state = layer1_public::rest::AgentRoutesState::new(
                layer1_public::agent_page::AgentPageOrigin::new("http://localhost").unwrap(),
                agents,
                reads,
                ops.clone(),
                operations.clone(),
            );
            #[cfg(feature = "qualification-hooks")]
            let state = if let Some(checkpoints) = &checkpoints {
                state.with_control_checkpoint(checkpoints[i].clone())
            } else {
                state
            };
            let routes = layer1_public::rest::agent_router(state);
            let entered = Arc::new(AtomicUsize::new(0));
            let counter = entered.clone();
            let routes = routes.layer(axum::middleware::from_fn(
                move |req: axum::extract::Request, next: axum::middleware::Next| {
                    let counter = counter.clone();
                    async move {
                        counter.fetch_add(1, Ordering::SeqCst);
                        assert!(
                            layer2_core::agent_ownership::execution_authority().is_none(),
                            "public durable control must not enter owner execution scope"
                        );
                        next.run(req).await
                    }
                },
            ));
            let router = routes::compose(
                coordinator.clone(),
                &updates,
                routes,
                store.clone(),
                h.adapter.clone(),
                identity,
            );
            let address = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let worker = AgentWorker::new(
                AgentWorkerDependencies {
                    store: store.clone(),
                    adapter: h.adapter.clone(),
                    skills: Arc::new(EmptyAgentSkillResolver),
                    registry: live,
                    updates,
                    operations: ops,
                },
                AgentWorkerConfig::new(format!("worker-{i}"), DEPLOYMENT_ID)
                    .with_ownership(Some(coordinator.clone())),
            );
            replicas.push(Replica {
                coordinator,
                worker,
                address,
                entered,
                server,
            });
        }
        failed_commands.store(0, Ordering::SeqCst);
        commands.lock().unwrap().clear();
        measurements::settle(&usage).await;
        usage.lock().unwrap().reset();
        Self {
            h,
            admin,
            name,
            app_name,
            ownership,
            replicas,
            owner,
            authority,
            agent,
            deleting,
            cancel_after_fence,
            failed_commands,
            commands,
            usage,
            ownership_io,
            cancel_after_admission,
        }
    }
}
