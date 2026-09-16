use super::lifecycle_fixture::LifecycleFixture;
use crate::store::{MongoBridgeInvokeStore, MongoChannelCommandStore};
use layer2_core::{
    bridge::{ingress::StoredInbox, invoke::*},
    resource_access::ResourceAccessService,
    sandbox::agent_io::*,
};
use std::sync::Arc;

pub(crate) struct QueuedChannelFixture {
    pub invokes: Arc<BridgeInvokeService>,
    pub store: Arc<MongoBridgeInvokeStore>,
    pub channel: Arc<ChannelAgentCommandService>,
    pub command: AdmittedAgentCommand,
    pub inbox: StoredInbox,
}

impl LifecycleFixture {
    /// Real scoped grant and signed callback, retained as a released queued
    /// command. No claim, prompt, revive or runtime dispatch occurs here.
    pub async fn queued_channel_command(&self, message_id: &str) -> QueuedChannelFixture {
        use super::super::super::*;
        let f = &self.f.original;
        let authority = renew_retained(&f.svc, &f.bridges, &f.record).await.unwrap();
        let store = Arc::new(
            MongoBridgeInvokeStore::open(f.bridges.clone(), f.resources.clone(), &f.database)
                .await
                .unwrap(),
        );
        let invokes = Arc::new(BridgeInvokeService::new(
            f.svc.clone(),
            Arc::new(ResourceAccessService::new(
                f.resources.clone(),
                Arc::new(Authority),
            )),
            f.resources.clone(),
            store.clone(),
        ));
        let grant = invokes
            .issue_channel(f.lease.clone(), &f.record.id, &f.record.pin, authority)
            .await
            .unwrap();
        let inbox_helpers =
            crate::store::bridge_definitions::mongo_integration_tests::inbox::callback;
        let (body, token, _) = inbox_helpers(
            message_id,
            "Continue the same Channel conversation after revival.",
        );
        let write = crate::store::bridge_definitions::mongo_integration_tests::inbox::write(
            &f.bridges,
            &f.record.pin.bridge_id,
            &body,
            &token,
        )
        .await;
        let inbox = write.record().clone();
        assert_eq!(inbox.pin, f.record.pin);
        let inbox_store = crate::store::MongoCallbackInbox::open(
            &f.bridges,
            &f.database,
            "fixture-deployment",
            &custody_key(),
        )
        .await
        .unwrap();
        inbox_store.accept(write).await.unwrap();
        let channel = Arc::new(ChannelAgentCommandService::new(Arc::new(
            MongoChannelCommandStore::new(store.clone()),
        )));
        let proof = invokes
            .authorize(BridgeInvokeRequest {
                grant_id: grant.id.clone(),
                pin: inbox.pin.clone(),
                target: f.record.placement.target().clone(),
                effect: BridgeInvokeEffect::AdmitMessage {
                    inbox_id: inbox.id.clone(),
                },
            })
            .await
            .unwrap();
        let held = channel
            .admit(proof, inbox.clone(), f.record.clone())
            .await
            .unwrap();
        let execution = channel.authorize_execution(&invokes, &held).await.unwrap();
        channel
            .release_execution_hold(&execution, &held)
            .await
            .unwrap();
        let command = f
            .resources
            .get_command(&held.receipt.envelope.command_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(command.status, AgentResponseStatus::Queued);
        assert!(command.work.lease.is_none() && command.work.dispatch.is_none());
        QueuedChannelFixture {
            invokes,
            store,
            channel,
            command,
            inbox,
        }
    }
}
