use super::*;
use futures_util::future::BoxFuture;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::*;
use layer2_core::{PlatformDefaults, RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::{ConversationInput, ConversationInputSender};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

pub(super) type Hook = Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send>;
struct NoHuman;
#[async_trait::async_trait]
impl AgentCommandAuthorizer for NoHuman {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<layer2_core::Caller, CommandAuthorizationError> {
        panic!("Channel control must never manufacture human authority")
    }
}
struct Runtime {
    sender: ConversationInputSender,
    resolves: AtomicUsize,
    hook_at: usize,
    hook: Mutex<Option<Hook>>,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("Channel controls cannot provision, revive or replace a runtime")
    }
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        let hook = if self.resolves.fetch_add(1, Ordering::SeqCst) == self.hook_at {
            self.hook.lock().unwrap().take()
        } else {
            None
        };
        if let Some(hook) = hook {
            hook().await;
        }
        Ok(AgentRuntimeConnection {
            capabilities: capabilities(),
            input: self.sender.clone(),
        })
    }
}
fn capabilities() -> RuntimeCapabilities {
    RuntimeCapabilities::new([
        RuntimeCapability::Conversation,
        RuntimeCapability::Steer,
        RuntimeCapability::Interrupt,
    ])
}
pub(super) fn service(c: &ControlFixture) -> Arc<ChannelControlExecutionService> {
    Arc::new(ChannelControlExecutionService::new(Arc::new(
        crate::store::bridge_invokes::channel_control_execution::MongoChannelControlExecutionStore::new(c.invokes.clone()),
    ), c.f.invokes.clone()))
}
pub(super) fn execution(
    c: &ControlFixture,
    hook: Option<Hook>,
    ambiguous: bool,
) -> (
    Arc<crate::commands::ResourceCommandExecution>,
    Arc<Mutex<Vec<ConversationInput>>>,
) {
    execution_at(c, hook, 1, ambiguous)
}
pub(super) fn execution_at(
    c: &ControlFixture,
    hook: Option<Hook>,
    hook_at: usize,
    ambiguous: bool,
) -> (
    Arc<crate::commands::ResourceCommandExecution>,
    Arc<Mutex<Vec<ConversationInput>>>,
) {
    let (dependencies, writes) = dependencies(c, hook, hook_at, ambiguous);
    (
        Arc::new(crate::commands::ResourceCommandExecution {
            channel_controls: Some(service(c)),
            channel_revival: dependencies.channel_revival,
            channel: Some(dependencies.channel),
            invokes: Some(Arc::new(c.f.invokes.clone())),
            authority: dependencies.authority,
            access: dependencies.access,
            store: dependencies.store,
            operations: dependencies.operations,
            context: dependencies.context,
        }),
        writes,
    )
}
pub(super) fn dependencies(
    c: &ControlFixture,
    hook: Option<Hook>,
    hook_at: usize,
    ambiguous: bool,
) -> (
    crate::commands::ProductionCommandDependencies,
    Arc<Mutex<Vec<ConversationInput>>>,
) {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let observed = writes.clone();
    let sender = ConversationInputSender::spawn(4, move |input| {
        observed.lock().unwrap().push(input);
        async move {
            if ambiguous {
                Err(layer3_adapter::AdapterError::Unavailable(
                    "owned writer lost acknowledgement after effect".into(),
                ))
            } else {
                Ok(())
            }
        }
    });
    let runtime = Arc::new(Runtime {
        sender,
        resolves: AtomicUsize::new(0),
        // These fixtures start held. Keep hooks at their original dispatch
        // boundary after the new hold-release and claim-readiness lookups.
        hook_at: hook_at + 2,
        hook: Mutex::new(hook),
    });
    (dependencies_with_runtime(c, runtime), writes)
}

pub(super) fn dependencies_with_runtime<R: AgentRuntimeRegistry + 'static>(
    c: &ControlFixture,
    runtime: Arc<R>,
) -> crate::commands::ProductionCommandDependencies {
    let profiles = Arc::new(InMemoryProfileStore::default());
    let context = AgentContextService::new(
        SandboxService::new(
            c.f.resources.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(c.f.resources.clone(), profiles.clone()),
        AgentService::new(
            c.f.resources.clone(),
            profiles.clone(),
            runtime.clone(),
            capabilities(),
        ),
        profiles,
        Arc::new(PlatformDefaults::default()),
    );
    crate::commands::ProductionCommandDependencies {
        channel_revival: None,
        channel: Arc::new(ChannelAgentCommandService::new(Arc::new(
            crate::store::MongoChannelCommandStore::new(c.invokes.clone()),
        ))),
        authority: Arc::new(NoHuman),
        access: Arc::new(ResourceAccessService::new(
            c.f.resources.clone(),
            Arc::new(Authority),
        )),
        store: c.f.resources.clone(),
        operations: AgentOperationService::new(c.f.resources.clone(), runtime),
        context,
    }
}
pub(super) fn operations(c: &ControlFixture) -> Arc<AgentOperationService> {
    dependencies(c, None, 0, false).0.operations
}

pub(super) async fn admit(c: &ControlFixture, steer: bool, suffix: &str) -> AdmittedChannelControl {
    c.f.resources
        .reconcile_command(&c.f.command.receipt.envelope.command_id)
        .await
        .unwrap();
    c.store
        .reserve(c.request(suffix, 200, steer).await)
        .await
        .unwrap()
}
pub(super) async fn claim(
    c: &ControlFixture,
    held: &AdmittedChannelControl,
) -> AdmittedAgentCommand {
    service(c)
        .release_held(&held.command, &operations(c))
        .await
        .unwrap();
    let now = std::time::SystemTime::now();
    c.f.resources
        .claim_next_command(CommandClaimRequest {
            agent_id: held.command.receipt.envelope.agent_id.clone(),
            holder: "owned-channel-control".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap()
}
pub(super) async fn run(
    c: &ControlFixture,
    held: &AdmittedChannelControl,
    execution: Arc<crate::commands::ResourceCommandExecution>,
) {
    AgentCommandWorker::new(
        c.f.resources.clone(),
        execution,
        "owned-control-worker".into(),
    )
    .unwrap()
    .run_one(&held.sidecar.command_id)
    .await
    .unwrap();
}
