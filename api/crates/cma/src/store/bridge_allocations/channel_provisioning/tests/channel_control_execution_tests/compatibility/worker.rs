use super::*;
use layer2_core::profiles::InMemoryProfileStore;
use layer2_core::resource_access::ResourceAccessService;
use layer2_core::sandbox::*;
use layer2_core::{PlatformDefaults, RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::{ConversationInput, ConversationInputSender};

struct NoHuman;
#[async_trait::async_trait]
impl AgentCommandAuthorizer for NoHuman {
    async fn authorize_command(
        &self,
        _: &AgentCommandEnvelope,
    ) -> Result<layer2_core::Caller, CommandAuthorizationError> {
        panic!("a retained Channel control cannot acquire human execution authority")
    }
}
struct Runtime {
    input: ConversationInputSender,
    conversation: String,
    generation: u64,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("compatibility must not provision, revive or replace the retained runtime")
    }
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        assert_eq!(target.conversation_id, self.conversation);
        assert_eq!(target.runtime_generation, self.generation);
        Ok(AgentRuntimeConnection {
            capabilities: capabilities(),
            input: self.input.clone(),
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
pub(super) fn execution(h: &Harness) -> Arc<crate::commands::ResourceCommandExecution> {
    let effects = h
        .client
        .database(&h.scenario.database)
        .collection::<Document>("compatibility_runtime_inputs");
    let command_id = h.scenario.control_command_id.clone();
    let run_id = h.scenario.run_id.clone();
    let ambiguous = h.scenario.mode == "unknown-write";
    let runtime = Arc::new(Runtime {
        conversation: h.scenario.runtime_conversation.clone(),
        generation: h.scenario.runtime_generation,
        input: ConversationInputSender::spawn(4, move |input| {
            let effects = effects.clone();
            let command_id = command_id.clone();
            let run_id = run_id.clone();
            async move {
                let kind = match input {
                    ConversationInput::Cancel => "interrupt",
                    ConversationInput::Prompt(_) => "steer",
                    _ => panic!("unexpected control effect"),
                };
                effects
                    .insert_one(doc! {"run_id":run_id,"command_id":command_id.as_str(),"kind":kind})
                    .await
                    .unwrap();
                if ambiguous {
                    Err(layer3_adapter::AdapterError::Unavailable(
                        "owned compatibility writer lost acknowledgement after effect".into(),
                    ))
                } else {
                    Ok(())
                }
            }
        }),
    });
    let profiles = Arc::new(InMemoryProfileStore::default());
    let context = AgentContextService::new(
        SandboxService::new(
            h.resources.clone(),
            profiles.clone(),
            SandboxPolicy::default(),
            Arc::new(FixedSandboxCredentialPrerequisite::Ready),
        ),
        WorkspaceService::new(h.resources.clone(), profiles.clone()),
        AgentService::new(
            h.resources.clone(),
            profiles.clone(),
            runtime.clone(),
            capabilities(),
        ),
        profiles,
        Arc::new(PlatformDefaults::default()),
    );
    Arc::new(crate::commands::ResourceCommandExecution {
        channel_controls: Some(service(h)),
        channel_revival: None,
        channel: Some(Arc::new(ChannelAgentCommandService::new(Arc::new(
            crate::store::MongoChannelCommandStore::new(h.store.clone()),
        )))),
        invokes: Some(Arc::new(h.invokes.clone())),
        authority: Arc::new(NoHuman),
        access: Arc::new(ResourceAccessService::new(
            h.resources.clone(),
            Arc::new(Authority),
        )),
        store: h.resources.clone(),
        operations: AgentOperationService::new(h.resources.clone(), runtime),
        context,
    })
}

pub(super) fn service(h: &Harness) -> Arc<ChannelControlExecutionService> {
    Arc::new(ChannelControlExecutionService::new(Arc::new(
        crate::store::bridge_invokes::channel_control_execution::MongoChannelControlExecutionStore::new(h.store.clone()),
    ), h.invokes.clone()))
}
