use super::*;
use layer2_core::bridge::invoke::*;
use layer2_core::sandbox::{
    AgentOperationService, AgentRuntimeConnection, AgentRuntimeError, AgentRuntimeRegistry,
    AgentRuntimeTarget,
};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::{ConversationInput, ConversationInputSender};
use mongodb::bson::{doc, Document};
use std::sync::Mutex;

struct Runtime {
    sender: ConversationInputSender,
}
#[async_trait::async_trait]
impl AgentRuntimeRegistry for Runtime {
    async fn resolve(
        &self,
        _: &AgentRuntimeTarget,
    ) -> Result<AgentRuntimeConnection, AgentRuntimeError> {
        Ok(AgentRuntimeConnection {
            capabilities: RuntimeCapabilities::new([
                RuntimeCapability::Conversation,
                RuntimeCapability::Interrupt,
            ]),
            input: self.sender.clone(),
        })
    }
    async fn fence(&self, _: &layer2_core::AgentId, _: u64) -> Result<(), AgentRuntimeError> {
        panic!("supersession cannot replace a runtime")
    }
}

fn operations(
    f: &ReplyFixture,
) -> (
    Arc<AgentOperationService>,
    Arc<Mutex<Vec<ConversationInput>>>,
) {
    let writes = Arc::new(Mutex::new(Vec::new()));
    let observed = writes.clone();
    let sender = ConversationInputSender::spawn(4, move |input| {
        observed.lock().unwrap().push(input);
        async { Ok(()) }
    });
    (
        AgentOperationService::new(f.resources.clone(), Arc::new(Runtime { sender })),
        writes,
    )
}

async fn admit(
    f: &ReplyFixture,
    message: &str,
) -> (ChannelAgentCommandService, AdmittedAgentCommand) {
    use crate::store::bridge_definitions::mongo_integration_tests::inbox;
    let (body, token, _) = if f.inbox.envelope.conversation_type == "private" {
        inbox::private_callback(message, "Stop that and handle this new request")
    } else {
        inbox::callback(message, "Stop that and handle this new request")
    };
    let write = inbox::write(&f.bridges, &f.inbox.pin.bridge_id, &body, &token).await;
    let input = write.record().clone();
    let callbacks = crate::store::MongoCallbackInbox::open(
        &f.bridges,
        &f.database,
        "fixture-deployment",
        &custody_key(),
    )
    .await
    .unwrap();
    callbacks.accept(write).await.unwrap();
    let store = Arc::new(
        crate::store::MongoBridgeInvokeStore::open(
            f.bridges.clone(),
            f.resources.clone(),
            &f.database,
        )
        .await
        .unwrap(),
    );
    let service = ChannelAgentCommandService::new(Arc::new(
        crate::store::MongoChannelCommandStore::new(store),
    ));
    let proof = f
        .invokes
        .authorize(BridgeInvokeRequest {
            grant_id: f.grant.id.clone(),
            pin: input.pin.clone(),
            target: f.allocation.placement.target().clone(),
            effect: BridgeInvokeEffect::AdmitMessage {
                inbox_id: input.id.clone(),
            },
        })
        .await
        .unwrap();
    let held = service
        .admit(proof, input, f.allocation.clone())
        .await
        .unwrap();
    (service, held)
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_supersession_interrupts_once_and_preserves_new_input_for_next_turn() {
    let f = ReplyFixture::private().await;
    let (channel, held) = admit(&f, "new-private-request").await;
    let authority = channel
        .authorize_execution(&f.invokes, &held)
        .await
        .unwrap();
    let (operations, writes) = operations(&f);
    let (first, duplicate) = tokio::join!(
        operations.interrupt_channel_for_input(&authority, &held),
        operations.interrupt_channel_for_input(&authority, &held),
    );
    assert!(
        first.is_ok() || duplicate.is_ok(),
        "{first:?}, {duplicate:?}"
    );
    assert!(matches!(
        writes.lock().unwrap().as_slice(),
        [ConversationInput::Cancel]
    ));
    assert_eq!(
        f.resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap(),
        held
    );
    let current = channel
        .authorize_execution(&f.invokes, &held)
        .await
        .unwrap();
    operations
        .interrupt_channel_for_input(&current, &held)
        .await
        .unwrap();
    assert_eq!(writes.lock().unwrap().len(), 1);
    let row = f
        .client
        .database(&f.database)
        .collection::<Document>("agent_commands")
        .find_one(doc! {"_id":held.receipt.envelope.command_id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.get_document("channel_supersession")
            .unwrap()
            .get_i64("turn_sequence")
            .unwrap(),
        f.command.work.dispatch.as_ref().unwrap().turn_sequence as i64
    );
    f.finish().await;
    let current = channel
        .authorize_execution(&f.invokes, &held)
        .await
        .unwrap();
    channel
        .release_execution_hold(&current, &held)
        .await
        .unwrap();
    f.resources
        .reconcile_command(&f.command.receipt.envelope.command_id)
        .await
        .unwrap();
    let now = std::time::SystemTime::now();
    let claimed = f
        .resources
        .claim_next_command(CommandClaimRequest {
            agent_id: held.receipt.envelope.agent_id.clone(),
            holder: "replacement-worker".into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.receipt, held.receipt);
    let current = channel
        .authorize_execution(&f.invokes, &claimed)
        .await
        .unwrap();
    operations
        .execute_channel_claim(&current, &claimed)
        .await
        .unwrap();
    assert!(matches!(
        &writes.lock().unwrap()[1],
        ConversationInput::RecoverablePrompt { .. }
    ));
    f.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_supersession_rejects_stale_active_turn_and_group_conversation() {
    for private in [true, false] {
        let f = if private {
            ReplyFixture::private().await
        } else {
            ReplyFixture::new().await
        };
        let (channel, held) = admit(&f, "new-isolated-request").await;
        let authority = channel
            .authorize_execution(&f.invokes, &held)
            .await
            .unwrap();
        if private {
            f.finish().await;
        }
        let (operations, writes) = operations(&f);
        let _ = operations
            .interrupt_channel_for_input(&authority, &held)
            .await;
        assert!(writes.lock().unwrap().is_empty());
        let row = f
            .client
            .database(&f.database)
            .collection::<Document>("agent_commands")
            .find_one(doc! {"_id":held.receipt.envelope.command_id.as_str()})
            .await
            .unwrap()
            .unwrap();
        assert!(!row.contains_key("channel_supersession"));
        f.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn channel_supersession_requires_exact_original_channel_source() {
    let f = ReplyFixture::private().await;
    let (channel, held) = admit(&f, "new-source-fenced-request").await;
    let authority = channel
        .authorize_execution(&f.invokes, &held)
        .await
        .unwrap();
    f.client
        .database(&f.database)
        .collection::<Document>("agent_commands")
        .update_one(
            doc! {"_id":f.command.receipt.envelope.command_id.as_str()},
            doc! {"$unset":{"channel_admission":""}},
        )
        .await
        .unwrap();
    let (operations, writes) = operations(&f);
    assert!(operations
        .interrupt_channel_for_input(&authority, &held)
        .await
        .is_err());
    assert!(writes.lock().unwrap().is_empty());
    f.close().await;
}
