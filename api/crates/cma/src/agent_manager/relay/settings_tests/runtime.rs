use super::*;
use layer2_core::sandbox::*;
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use std::sync::atomic::{AtomicBool, Ordering};

pub(super) struct Runtime {
    pub revocation: Mutex<
        Option<(
            Arc<layer2_core::resource_access::ResourceAccessService>,
            layer2_core::resource_access::ResourceShare,
        )>,
    >,
    pub target: Mutex<Option<AgentRuntimeTarget>>,
    pub resolutions: Mutex<Vec<(AgentRuntimeTarget, Option<OwnershipToken>)>>,
    pub inputs: Arc<Mutex<Vec<layer3_adapter::ConversationInput>>>,
    pub ambiguous: Arc<AtomicBool>,
    store: Arc<MongoSandboxStore>,
}

impl Runtime {
    pub fn new(store: Arc<MongoSandboxStore>) -> Self {
        Self {
            revocation: Mutex::new(None),
            target: Mutex::new(None),
            resolutions: Mutex::default(),
            inputs: Arc::default(),
            ambiguous: Arc::default(),
            store,
        }
    }

    pub fn attach(&self) {
        *self.target.lock().unwrap() = Some(AgentRuntimeTarget {
            agent_id: AgentId(AGENT.into()),
            conversation_id: "conversation-settings".into(),
            runtime_generation: 1,
        });
    }
}

#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        self.resolutions.lock().unwrap().push((
            target.clone(),
            layer2_core::agent_ownership::execution_owner(),
        ));
        let revoke = self.revocation.lock().unwrap().take();
        if let Some((access, share)) = revoke {
            super::access::revoke(&access, &share).await;
        }
        let retained = self.target.lock().unwrap().clone();
        let Some(retained) = retained else {
            return Err(AgentRuntimeError::NotFound);
        };
        if retained.agent_id != target.agent_id
            || retained.conversation_id != target.conversation_id
        {
            return Err(AgentRuntimeError::NotFound);
        }
        if retained.runtime_generation != target.runtime_generation {
            return Err(AgentRuntimeError::StaleGeneration);
        }
        let inputs = self.inputs.clone();
        let store = self.store.clone();
        let id = target.agent_id.clone();
        let ambiguous = self.ambiguous.clone();
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input: AgentRuntimeInputSender::spawn(4, move |input| {
                let inputs = inputs.clone();
                let store = store.clone();
                let id = id.clone();
                let ambiguous = ambiguous.clone();
                async move {
                    let admitted = store.get_agent(&id).await.unwrap().unwrap();
                    assert!(
                        admitted.settings_pending(),
                        "Configure requires durable admission"
                    );
                    assert_eq!(
                        configuration(&input),
                        configuration(&admitted.settings().runtime_input())
                    );
                    inputs.lock().unwrap().push(input);
                    if ambiguous.load(Ordering::SeqCst) {
                        Err(layer3_adapter::AdapterError::Unavailable(
                            "confirmation lost".into(),
                        ))
                    } else {
                        Ok(())
                    }
                }
            }),
        })
    }

    async fn fence(&self, _: &AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        Ok(())
    }
}

fn configuration(
    input: &layer3_adapter::ConversationInput,
) -> (
    &str,
    layer3_adapter::ReasoningEffort,
    layer3_adapter::PermissionLevel,
    Option<&str>,
) {
    match input {
        layer3_adapter::ConversationInput::Configure {
            model,
            reasoning_effort,
            permission,
            system_prompt,
        } => (
            model,
            *reasoning_effort,
            *permission,
            system_prompt.as_deref(),
        ),
        _ => panic!("settings must send Configure"),
    }
}

#[async_trait::async_trait]
impl SandboxResourceReader for Runtime {
    async fn sandbox_resources(
        &self,
        _: &Sandbox,
    ) -> Result<SandboxResourceReading, SandboxResourceReadError> {
        panic!("settings cannot read runtime resource usage")
    }
}
