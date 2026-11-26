use super::*;
use axum::{
    extract::Path,
    http::{HeaderName, HeaderValue, Method},
    middleware::{self, Next},
    routing::{get, post},
    Json,
};
use layer2_core::agent_ownership::execution_owner;
use std::sync::Mutex;
use tokio::{io::AsyncReadExt, sync::watch};

type Receiver = Arc<Mutex<Vec<Value>>>;

pub(super) async fn serve(mongo: &Client, name: &str) {
    let instance = uuid::Uuid::new_v4().simple().to_string();
    let store = Arc::new(
        MongoSandboxStore::open(mongo, name)
            .await
            .unwrap()
            .enable_agent_ownership(instance.clone())
            .await
            .unwrap(),
    );
    let agent = store
        .get_agent(&AgentId("agt_sw04-cancel".into()))
        .await
        .unwrap()
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let coordinator = OwnershipCoordinator::register(
        store.agent_ownership.clone().unwrap(),
        Member {
            instance_id: instance.clone(),
            protocol: PROTOCOL_VERSION,
            relay_address: format!("http://{address}"),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 8,
        },
    )
    .await
    .unwrap();
    let updates = DurableAgentUpdateRecorder::new(store.clone());
    let relay = Relay::new(
        coordinator.clone(),
        &updates,
        ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-sw04-relay"),
    )
    .unwrap();
    let access = Arc::new(ResourceAccessService::new(
        store.clone(),
        Arc::new(fixture::ReadOnly),
    ));
    let reads = AgentReadService::new(store.clone(), Arc::new(fixture::ReadOnly))
        .with_resource_access(access);
    let context = fixture::context(agent.owner.clone());
    let public = containment::controls_router(store.clone(), reads)
        .layer(Extension(layer1_public::AuthenticatedSubject(agent.owner)))
        .layer(Extension(context.caller.authorization.clone()))
        .layer(Extension(context.clone()))
        .layer(Extension(layer1_public::DelegatedCredential::new(
            "stream-fixture".into(),
        )));

    let process_hash = hash(&instance);
    let header = HeaderValue::from_str(&process_hash).unwrap();
    let public = relay.routes(public).layer(middleware::from_fn(
        move |request: axum::extract::Request, next: Next| {
            let header = header.clone();
            async move {
                let observe =
                    request.method() == Method::POST && request.uri().path().ends_with("/controls");
                let mut response = next.run(request).await;
                if observe {
                    response
                        .headers_mut()
                        .insert(HeaderName::from_static(PROCESS_HEADER), header);
                }
                response
            }
        },
    ));
    let receiver = Receiver::default();
    let execution = Arc::new(Execution {
        store: store.clone(),
        caller: context.caller,
        process_hash,
        receiver: receiver.clone(),
    });
    let worker = AgentCommandWorker::new(store, execution, instance.clone())
        .unwrap()
        .with_ownership(Some(coordinator.clone()));
    let acquire = coordinator.clone();
    let app = crate::with_request_context(
        public
            .route(
                "/fixture/acquire/{agent}",
                post(move |Path(agent): Path<String>| async move {
                    if acquire.acquire(&AgentId(agent)).await.unwrap().is_some() {
                        StatusCode::NO_CONTENT
                    } else {
                        StatusCode::CONFLICT
                    }
                }),
            )
            .route(
                "/fixture/drive/{command}",
                post(move |Path(command): Path<String>| async move {
                    worker
                        .run_scheduled(&AgentCommandId(command))
                        .await
                        .unwrap();
                    StatusCode::NO_CONTENT
                }),
            )
            .route(
                "/fixture/effects",
                get(move || async move { Json(receiver.lock().unwrap().clone()) }),
            ),
    );
    println!(
        "{READY}{}",
        json!({"pid": std::process::id(), "address": address, "instance": instance})
    );
    let (stop, draining) = watch::channel(());
    let stdin_closed = async move {
        let mut stdin = tokio::io::stdin();
        assert_eq!(stdin.read(&mut [0]).await.expect("read fixture stdin"), 0);
        let _ = stop.send(());
    };
    let server = async {
        axum::serve(listener, app)
            .with_graceful_shutdown(stdin_closed)
            .await
            .expect("SW04 test listener failed");
    };
    tokio::join!(server, maintain(coordinator, draining));
}

async fn maintain(coordinator: Arc<OwnershipCoordinator>, mut draining: watch::Receiver<()>) {
    let mut ticks = tokio::time::interval(Duration::from_millis(HEARTBEAT_INTERVAL_MS));
    loop {
        tokio::select! {
            biased;
            _ = draining.changed() => return,
            _ = ticks.tick() => {}
        }
        coordinator
            .maintain()
            .await
            .expect("fixture ownership renewal failed");
    }
}

struct Execution {
    store: Arc<MongoSandboxStore>,
    caller: Caller,
    process_hash: String,
    receiver: Receiver,
}

#[async_trait::async_trait]
impl AgentCommandExecution for Execution {
    async fn check_queued(&self, command: &AdmittedAgentCommand) -> CommandReadiness {
        if command.receipt.envelope.owner
            == (AgentOwner::Person {
                subject: self.caller.subject.clone(),
            })
        {
            CommandReadiness::Ready
        } else {
            CommandReadiness::Reject(CommandFailureCode::AuthorityRevoked)
        }
    }

    async fn prepare_queued(
        &self,
        _: &AdmittedAgentCommand,
    ) -> Result<(), CommandPreparationError> {
        Err(CommandPreparationError::Rejected(
            CommandFailureCode::AgentUnavailable,
        ))
    }

    async fn execute(&self, command: &AdmittedAgentCommand) -> CommandAttempt {
        let runtime = Runtime {
            command_hash: hash(command.receipt.envelope.command_id.as_str()),
            process_hash: self.process_hash.clone(),
            receiver: self.receiver.clone(),
        };
        AgentOperationService::new(self.store.clone(), Arc::new(runtime))
            .execute_claimed_command(&self.caller, command)
            .await
            .expect("run the actual claimed Agent operation");
        CommandAttempt::Finished
    }
}

struct Runtime {
    command_hash: String,
    process_hash: String,
    receiver: Receiver,
}

#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        // The runtime input task is spawned after the authority lookup. Bind its
        // receiver to the command and observed owner granted to this attempt.
        let owner = execution_owner().expect("runtime resolution must run under ownership");
        assert_eq!(owner.agent_id, target.agent_id);
        let effect = json!({
            "kind": "interrupt", "processHash": self.process_hash,
            "commandHash": self.command_hash, "ownerProcessHash": hash(&owner.instance_id),
            "epoch": owner.epoch, "runtimeGeneration": target.runtime_generation,
        });
        let receiver = self.receiver.clone();
        Ok(AgentRuntimeConnection {
            capabilities: capabilities(),
            input: AgentRuntimeInputSender::spawn(4, move |input| {
                if matches!(input, AgentRuntimeInput::Cancel) {
                    receiver.lock().unwrap().push(effect.clone());
                }
                async { Ok(()) }
            }),
        })
    }

    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        unreachable!("active-response Cancel must not retire a runtime")
    }
}
