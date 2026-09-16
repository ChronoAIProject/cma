//! Exact retained Channel controls. Production composition deliberately does
//! not construct this execution adapter until accepted ingress rollout is proved.
use super::MongoBridgeInvokeStore;
use crate::store::sandbox_mongo::{record_bson, start_transaction};
use layer2_core::bridge::channel_control::ChannelControlError as Error;
use layer2_core::sandbox::agent_io::*;
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};
use std::sync::Arc;

mod fence;
mod read;
mod release;

pub(crate) struct MongoChannelControlExecutionStore {
    invokes: Arc<MongoBridgeInvokeStore>,
}
impl MongoChannelControlExecutionStore {
    #[allow(dead_code)] // Only owned fixtures configure execution in this foundation.
    pub(crate) fn new(invokes: Arc<MongoBridgeInvokeStore>) -> Self {
        invokes
            .resources
            .channel_invokes
            .get_or_init(|| Arc::downgrade(&invokes));
        Self { invokes }
    }
}
#[async_trait::async_trait]
impl ChannelControlExecutionStore for MongoChannelControlExecutionStore {
    async fn read_control_execution(
        &self,
        command: &AdmittedAgentCommand,
        deadline: tokio::time::Instant,
    ) -> Result<ChannelControlExecutionSnapshot, Error> {
        let work = async {
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| Error::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| Error::Unavailable)?;
            let result = self
                .invokes
                .read_control_execution(command, deadline, &mut session)
                .await;
            let _ = session.abort_transaction().await;
            result.map_err(MutationError::into_core)
        };
        tokio::time::timeout_at(deadline, work)
            .await
            .map_err(|_| Error::Unavailable)?
    }
    async fn release_control_hold(
        &self,
        fence: Arc<ChannelControlExecutionFence>,
        command: &AdmittedAgentCommand,
    ) -> Result<(), Error> {
        self.release(&fence, command).await
    }
    async fn expire_control_hold(&self, command: &AdmittedAgentCommand) -> Result<(), Error> {
        self.expire(command).await
    }
}

pub(crate) enum MutationError {
    Core(Error),
    Driver(mongodb::error::Error),
}
impl MutationError {
    pub(super) fn into_core(self) -> Error {
        match self {
            Self::Core(error) => error,
            Self::Driver(_) => Error::Unavailable,
        }
    }
}
impl From<Error> for MutationError {
    fn from(value: Error) -> Self {
        Self::Core(value)
    }
}
impl From<mongodb::error::Error> for MutationError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<super::MutationError> for MutationError {
    fn from(value: super::MutationError) -> Self {
        match value {
            super::MutationError::Mongo(error) => Self::Driver(error),
            super::MutationError::Invoke(_) => Self::Core(Error::AuthorityUnavailable),
        }
    }
}
impl From<super::channel::MutationError> for MutationError {
    fn from(value: super::channel::MutationError) -> Self {
        match value {
            super::channel::MutationError::Driver(error) => Self::Driver(error),
            super::channel::MutationError::Core(_) => Self::Core(Error::SourceUnavailable),
        }
    }
}

impl From<crate::store::bridge_definitions::inbox::MutationError> for MutationError {
    fn from(value: crate::store::bridge_definitions::inbox::MutationError) -> Self {
        use crate::store::bridge_definitions::inbox::MutationError as CallbackMutation;
        use layer2_core::bridge::ingress::CallbackError;
        match value {
            CallbackMutation::Mongo(error) => Self::Driver(error),
            CallbackMutation::Core(CallbackError::ReplayConflict) => Self::Core(Error::Corrupt),
            CallbackMutation::Core(_) => Self::Core(Error::Unavailable),
        }
    }
}
