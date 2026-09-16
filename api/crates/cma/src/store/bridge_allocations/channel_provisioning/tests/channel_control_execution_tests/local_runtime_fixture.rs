use super::*;
use crate::config::bridge_flags::BridgeFlag;
use crate::sandbox::LiveAgentRegistry;
use layer2_core::sandbox::*;
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::{ConversationInput, ConversationInputSender, ConversationUpdate};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};

pub(super) struct LocalRuntime {
    pub registry: Arc<LiveAgentRegistry>,
    pub writes: Arc<Mutex<Vec<ConversationInput>>>,
    pub resolves: AtomicUsize,
    pub hook: Mutex<Option<(usize, worker::Hook)>>,
    target: AgentRuntimeTarget,
    attached_agent_id: layer2_core::AgentId,
    _updates: Option<tokio::sync::mpsc::Sender<ConversationUpdate>>,
}

impl LocalRuntime {
    pub fn new(
        c: &ControlFixture,
        held: &AdmittedChannelControl,
        mode: &str,
        ambiguous: bool,
    ) -> Arc<Self> {
        let registry = Arc::new(LiveAgentRegistry::default());
        let target = AgentRuntimeTarget {
            agent_id: held.command.receipt.envelope.agent_id.clone(),
            conversation_id: held.sidecar.intent.runtime_conversation_id.clone(),
            runtime_generation: held.sidecar.intent.runtime_generation,
        };
        let attached_agent_id = if mode == "agent" {
            layer2_core::AgentId("another-local-agent".into())
        } else {
            target.agent_id.clone()
        };
        let writes = Arc::new(Mutex::new(Vec::new()));
        let mut updates = None;
        if mode != "missing" {
            let observed = writes.clone();
            let sender = ConversationInputSender::spawn(4, move |input| {
                observed.lock().unwrap().push(input);
                async move {
                    if ambiguous {
                        Err(layer3_adapter::AdapterError::Unavailable(
                            "owned acknowledgement lost".into(),
                        ))
                    } else {
                        Ok(())
                    }
                }
            });
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            let capabilities = if mode == "capability" {
                RuntimeCapabilities::new([RuntimeCapability::Conversation])
            } else if mode == "conversation-capability" {
                RuntimeCapabilities::new([RuntimeCapability::Steer, RuntimeCapability::Interrupt])
            } else {
                RuntimeCapabilities::new([
                    RuntimeCapability::Conversation,
                    RuntimeCapability::Steer,
                    RuntimeCapability::Interrupt,
                ])
            };
            let attachment = registry
                .attach(
                    attached_agent_id.clone(),
                    if mode == "conversation" {
                        "another-retained-conversation".into()
                    } else {
                        target.conversation_id.clone()
                    },
                    target.runtime_generation + u64::from(mode == "generation"),
                    1,
                    capabilities,
                    sender,
                    rx,
                    DurableAgentUpdateRecorder::new(c.f.resources.clone()),
                )
                .unwrap();
            if mode != "unpublished" {
                registry.publish_and_activate(&attachment).unwrap();
            }
            updates = Some(tx);
        }
        Arc::new(Self {
            registry,
            target,
            attached_agent_id,
            writes,
            resolves: AtomicUsize::new(0),
            hook: Mutex::new(None),
            _updates: updates,
        })
    }

    pub fn detach(&self) {
        self.registry.fence(&self.attached_agent_id, None, 1);
    }
}

impl Drop for LocalRuntime {
    fn drop(&mut self) {
        self.detach();
    }
}

#[async_trait::async_trait]
impl AgentRuntimeRegistry for LocalRuntime {
    async fn resolve(
        &self,
        target: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        assert_eq!(
            target, &self.target,
            "readiness must use the exact retained source target"
        );
        let index = self.resolves.fetch_add(1, Ordering::SeqCst);
        let hook = {
            let mut pending = self.hook.lock().unwrap();
            if pending.as_ref().is_some_and(|(at, _)| *at == index) {
                pending.take().map(|(_, hook)| hook)
            } else {
                None
            }
        };
        if let Some(hook) = hook {
            hook().await;
        }
        AgentRuntimeRegistry::resolve(self.registry.as_ref(), target).await
    }

    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("control readiness cannot provision, revive or replace a runtime")
    }
}

pub(super) fn execution(
    c: &ControlFixture,
    runtime: Arc<LocalRuntime>,
) -> Arc<crate::commands::ResourceCommandExecution> {
    production::with_dependencies(
        c,
        &[BridgeFlag::Execution, BridgeFlag::Telegram],
        worker::dependencies_with_runtime(c, runtime),
    )
}

pub(super) fn worker(
    c: &ControlFixture,
    execution: Arc<dyn AgentCommandExecution>,
    holder: &str,
) -> Arc<AgentCommandWorker> {
    AgentCommandWorker::new(c.f.resources.clone(), execution, holder.into()).unwrap()
}

pub(super) async fn command(
    c: &ControlFixture,
    held: &AdmittedChannelControl,
) -> AdmittedAgentCommand {
    c.f.resources
        .get_command(&held.sidecar.command_id)
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn state(c: &ControlFixture, held: &AdmittedChannelControl) -> Value {
    let id = &held.command.receipt.envelope.agent_id;
    let database = c.f.client.database(&c.f.database);
    json!({
        "command":database.collection::<Document>("agent_commands").find_one(doc!{"_id":held.sidecar.command_id.as_str()}).await.unwrap(),
        "lane":database.collection::<Document>("agent_command_orders").find_one(doc!{"_id":id.as_str()}).await.unwrap(),
        "agent":c.f.resources.get_agent(id).await.unwrap(),
        "logs":c.f.resources.agent_log(id,0,100).await.unwrap(),
    })
}

pub(super) async fn later_message(c: &ControlFixture) -> AdmittedAgentCommand {
    let inbox_fixture = crate::store::bridge_definitions::mongo_integration_tests::inbox::callback;
    let (body, token, _) =
        inbox_fixture("later-local-readiness-message", "A later ordinary message");
    let write = crate::store::bridge_definitions::mongo_integration_tests::inbox::write(
        &c.f.bridges,
        &c.f.inbox.pin.bridge_id,
        &body,
        &token,
    )
    .await;
    let inbox = write.record().clone();
    c.callbacks.accept(write).await.unwrap();
    let service = ChannelAgentCommandService::new(Arc::new(
        crate::store::MongoChannelCommandStore::new(c.invokes.clone()),
    ));
    let proof =
        c.f.invokes
            .authorize(BridgeInvokeRequest {
                grant_id: c.f.grant.id.clone(),
                pin: inbox.pin.clone(),
                target: c.f.allocation.placement.target().clone(),
                effect: BridgeInvokeEffect::AdmitMessage {
                    inbox_id: inbox.id.clone(),
                },
            })
            .await
            .unwrap();
    let held = service
        .admit(proof, inbox, c.f.allocation.clone())
        .await
        .unwrap();
    let authority = service
        .authorize_execution(&c.f.invokes, &held)
        .await
        .unwrap();
    service
        .release_execution_hold(&authority, &held)
        .await
        .unwrap();
    c.f.resources
        .get_command(&held.receipt.envelope.command_id)
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn expire(c: &ControlFixture, held: &mut AdmittedChannelControl) {
    // Model elapsed queue time while preserving timestamp and digest invariants.
    let admitted = std::time::SystemTime::now() - Duration::from_secs(301);
    held.sidecar.intent.captured_at = admitted
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    held.sidecar.intent_sha256 = held.sidecar.intent.fingerprint().unwrap();
    held.command.receipt.admitted_at = layer2_core::rfc3339_at(admitted);
    held.command.queue_deadline = layer2_core::rfc3339_at(admitted + Duration::from_secs(300));
    held.validate().unwrap();
    let mut row = crate::store::agent_commands::command_document(&held.command).unwrap();
    row.insert(
        "channel_control_admission",
        record_bson(&held.sidecar).unwrap(),
    );
    let database = c.f.client.database(&c.f.database);
    database
        .collection::<Document>("agent_commands")
        .replace_one(doc! {"_id":held.sidecar.command_id.as_str()}, row)
        .await
        .unwrap();
    database
        .collection::<Document>("bridge_channel_control_intents")
        .update_one(
            doc! {"_id":&held.sidecar.intent.id},
            doc! {"$set":{
                "record":record_bson(&held.sidecar.intent).unwrap(),
                "intent_sha256":&held.sidecar.intent_sha256,
            }},
        )
        .await
        .unwrap();
}
