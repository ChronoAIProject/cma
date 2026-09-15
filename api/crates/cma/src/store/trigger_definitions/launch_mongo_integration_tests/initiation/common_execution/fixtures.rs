use super::*;

pub(super) struct Accepted {
    pub f: Fixture,
    pub launch: Launch,
    pub request: AgentAdmission,
    pub agent: Agent,
    pub command: AdmittedAgentCommand,
}
impl Accepted {
    pub async fn new() -> Self {
        let (f, launch, request) = pending().await;
        let created = f
            .resources
            .admit_agent_with_trigger_initiator(
                request.clone(),
                launch.initiation.clone().unwrap(),
                false,
            )
            .await
            .unwrap();
        let mut agent = created.creation().agent.clone();
        let binding = agent
            .conversation
            .first_message
            .as_ref()
            .unwrap()
            .common_binding
            .as_ref()
            .unwrap();
        let command = f
            .resources
            .get_command(&binding.receipt().envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        let launch = f
            .seed
            .store
            .launch(&launch.id, &launch.owner)
            .await
            .unwrap()
            .unwrap();
        binding.require_launch(&launch).unwrap();
        let now = layer2_core::now_rfc3339();
        agent.status = AgentStatus::Ready;
        agent.ready_at = Some(now.clone());
        agent.conversation.conversation_id = Some("common-trigger-conversation".into());
        agent.conversation.runtime_generation = Some(1);
        agent.work = AgentWorkState::new(AgentWorkStep::DispatchFirstMessage, now.clone());
        agent.work.claim("startup".into(), &now, expiry()).unwrap();
        agent.revision += 1;
        agent.updated_at = now;
        let tree_row = f
            .resources
            .worktrees
            .find_one(doc! {"_id":agent.worktree_id.as_str()})
            .await
            .unwrap()
            .unwrap();
        let mut tree = crate::store::worktrees::read_worktree(&tree_row).unwrap();
        tree.status = WorktreeStatus::Ready;
        tree.revision += 1;
        f.resources
            .worktrees
            .replace_one(
                doc! {"_id":tree.id.as_str()},
                crate::store::worktrees::worktree_document(&tree).unwrap(),
            )
            .await
            .unwrap();
        f.resources
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                crate::store::agents::agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        Self {
            f,
            launch,
            request,
            agent,
            command,
        }
    }
    pub async fn claim(&self) -> AdmittedAgentCommand {
        self.f
            .resources
            .claim_next_command(CommandClaimRequest {
                agent_id: self.agent.id.clone(),
                holder: "common-worker".into(),
                now: layer2_core::now_rfc3339(),
                expires_at: expiry(),
            })
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn current_agent(&self) -> Agent {
        self.f
            .resources
            .get_agent(&self.agent.id)
            .await
            .unwrap()
            .unwrap()
    }
    pub async fn current_command(&self) -> AdmittedAgentCommand {
        self.f
            .resources
            .reconcile_command(&self.command.receipt.envelope.command_id)
            .await
            .unwrap()
    }
}
pub(super) async fn pending() -> (Fixture, Launch, AgentAdmission) {
    let mut f = Fixture::new(false).await;
    f.service = f.service.with_common_first_messages(true);
    let launch = f
        .ready_for_agent(f.launch("common-first-command").await)
        .await;
    let mut request = f.admission(&launch).await;
    request.first_message = Some(
        FirstMessageRecord::new(
            request.creation_key.clone(),
            "Inspect the accepted project.",
            request.event_at.clone(),
        )
        .unwrap(),
    );
    (f, launch, request)
}

pub(super) fn expiry() -> String {
    layer2_core::rfc3339_at(std::time::SystemTime::now() + std::time::Duration::from_secs(60))
}

pub(super) struct Runtime {
    sender: layer3_adapter::ConversationInputSender,
    resolves: AtomicUsize,
    prewrite: bool,
    pub sends: Arc<AtomicUsize>,
}
impl Runtime {
    pub fn new(prewrite: bool, ambiguous: bool) -> Arc<Self> {
        let sends = Arc::new(AtomicUsize::new(0));
        let count = sends.clone();
        let sender = layer3_adapter::ConversationInputSender::spawn(4, move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                if ambiguous {
                    Err(layer3_adapter::AdapterError::Rejected(
                        "receipt lost after writer invocation".into(),
                    ))
                } else {
                    Ok(())
                }
            }
        });
        Arc::new(Self {
            sender,
            resolves: AtomicUsize::new(0),
            prewrite,
            sends,
        })
    }
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        if self.prewrite && self.resolves.fetch_add(1, Ordering::SeqCst) > 0 {
            return Err(AgentRuntimeError::Unavailable(
                "runtime disappeared before writer admission".into(),
            ));
        }
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: self.sender.clone(),
        })
    }
    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("initial input cannot retire runtime")
    }
}
