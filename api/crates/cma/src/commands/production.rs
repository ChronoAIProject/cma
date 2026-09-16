//! Retained command execution remains composed independently of fresh ingress.
use std::sync::Arc;

use layer2_core::{
    bridge::{invoke::BridgeInvokeService, BridgeConnectionValidator},
    resource_access::ResourceAccessService,
    sandbox::{
        agent_io::{
            AgentCommandAuthorizer, AgentCommandWorkerStore, ChannelAgentCommandService,
            ChannelControlExecutionService,
        },
        AgentContextService, AgentOperationService,
    },
};

use super::{ChannelCommandRevival, ResourceCommandExecution};
use crate::store::{MongoBridgeInvokeStore, MongoChannelControlExecutionStore};

pub(crate) struct ProductionCommandDependencies {
    pub channel_revival: Option<Arc<ChannelCommandRevival>>,
    pub channel: Arc<ChannelAgentCommandService>,
    pub authority: Arc<dyn AgentCommandAuthorizer>,
    pub access: Arc<ResourceAccessService>,
    pub store: Arc<dyn AgentCommandWorkerStore>,
    pub operations: Arc<AgentOperationService>,
    pub context: Arc<AgentContextService>,
}

impl ResourceCommandExecution {
    /// Every production worker can drain and reconcile accepted controls. Stage
    /// and platform gates apply only when a held control is released.
    pub(crate) fn production(
        dependencies: ProductionCommandDependencies,
        store: Arc<MongoBridgeInvokeStore>,
        invokes: BridgeInvokeService,
        gates: Arc<dyn BridgeConnectionValidator>,
    ) -> Arc<Self> {
        let controls = ChannelControlExecutionService::new(
            Arc::new(MongoChannelControlExecutionStore::new(store)),
            invokes.clone(),
        )
        .with_feature_gates(gates);
        Arc::new(Self {
            channel_controls: Some(Arc::new(controls)),
            channel_revival: dependencies.channel_revival,
            channel: Some(dependencies.channel),
            invokes: Some(Arc::new(invokes)),
            authority: dependencies.authority,
            access: dependencies.access,
            store: dependencies.store,
            operations: dependencies.operations,
            context: dependencies.context,
        })
    }
}
