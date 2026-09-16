//! Channel reservations share the Agent ledger/order and the current invoke
//! transaction. This adapter is deliberately separate from execution authority.

use std::sync::Arc;

use layer2_core::sandbox::agent_io::{
    AdmittedAgentCommand, ChannelCommandAdmissionError, ChannelCommandReservation,
    ChannelCommandStore, CommandReservationResult,
};
use mongodb::{bson::Document, ClientSession, Collection};

use crate::store::{
    agent_commands::{reserve, ApplyError},
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    },
};

use super::MongoBridgeInvokeStore;

mod acceptance_receipt;
mod execution;
mod refusal;
mod retained;
mod retry;

pub(crate) struct MongoChannelCommandStore {
    invokes: Arc<MongoBridgeInvokeStore>,
    inbox: Collection<Document>,
    allocations: Collection<Document>,
    acceptance_receipts: Collection<Document>,
}

impl MongoChannelCommandStore {
    /// Invoke storage already verifies both resource and bridge namespaces.
    /// No collections, indexes, execution engine or resource tree are created.
    pub(crate) fn new(invokes: Arc<MongoBridgeInvokeStore>) -> Self {
        invokes
            .resources
            .channel_invokes
            .get_or_init(|| Arc::downgrade(&invokes));
        let namespace = invokes.bridges.definitions.namespace();
        let database = invokes.bridges.client.database(&namespace.db);
        Self {
            invokes,
            inbox: majority_collection(&database, "bridge_callback_inbox"),
            allocations: majority_collection(&database, "bridge_allocations"),
            acceptance_receipts: majority_collection(&database, "bridge_callback_receipts"),
        }
    }

    async fn transaction(
        &self,
        request: &ChannelCommandReservation,
        session: &mut ClientSession,
    ) -> Result<AdmittedAgentCommand, MutationError> {
        request.require_current()?;
        // This fence includes fresh grant, bridge, custody and complete retained
        // target creation identity; it MUST run even for duplicate commands.
        self.invokes
            .fence_transaction(request.proof(), session)
            .await?;
        self.fence_retained(request, session).await?;
        let result =
            reserve::apply(&self.invokes.resources, request.reservation(), session).await?;
        let (record, created) = match result {
            CommandReservationResult::Created(record) => {
                request.require_admission_enabled()?;
                (record, true)
            }
            CommandReservationResult::Existing(record) => (record, false),
        };
        request.require_result(&record)?;
        self.bind_sidecar(request, created, session).await?;
        self.invokes
            .record_accepted_usage(request, &record, created, session)
            .await?;
        request.require_current()?;
        Ok(*record)
    }
}

#[async_trait::async_trait]
impl ChannelCommandStore for MongoChannelCommandStore {
    async fn refuse_channel_hold(
        &self,
        refusal: Arc<layer2_core::sandbox::agent_io::ChannelHoldRefusal>,
    ) -> Result<(), ChannelCommandAdmissionError> {
        self.refuse_held(refusal).await
    }
    async fn channel_execution_source(
        &self,
        command: &AdmittedAgentCommand,
    ) -> Result<
        layer2_core::sandbox::agent_io::ChannelCommandExecutionSource,
        ChannelCommandAdmissionError,
    > {
        self.read_execution_source(command).await
    }

    async fn release_channel_command(
        &self,
        fence: Arc<layer2_core::sandbox::agent_io::ChannelExecutionFence>,
        command: &AdmittedAgentCommand,
    ) -> Result<(), ChannelCommandAdmissionError> {
        self.release_held(fence, command).await
    }

    async fn reserve_channel_command(
        &self,
        request: ChannelCommandReservation,
    ) -> Result<AdmittedAgentCommand, ChannelCommandAdmissionError> {
        let mut retries = retry::ChannelAdmissionRetries::new(request.proof().deadline());
        while retries.next().await {
            request.require_current()?;
            let mut session = self
                .invokes
                .bridges
                .client
                .start_session()
                .await
                .map_err(|_| ChannelCommandAdmissionError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ChannelCommandAdmissionError::Unavailable)?;
            let record = match self.transaction(&request, &mut session).await {
                Ok(record) => record,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        MutationError::Driver(error)
                            if is_duplicate_key(&error) || is_transient(&error) =>
                        {
                            continue;
                        }
                        MutationError::Driver(_) => {
                            return Err(ChannelCommandAdmissionError::Unavailable);
                        }
                        MutationError::Core(error) => return Err(error),
                    }
                }
            };
            if let Err(error) = request.require_current() {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(record),
                CommitOutcome::Retry(_) => continue,
                // No runtime input was sent. A retry with fresh authority must
                // recover the same permanent command/response identity.
                CommitOutcome::Unknown(_) | CommitOutcome::Failed(_) => {
                    return Err(ChannelCommandAdmissionError::Unavailable);
                }
            }
        }
        // Retrying never renews the proof. Preserve an expired authority error;
        // elapsed transient contention itself is not a semantic ledger conflict.
        request.require_current()?;
        Err(ChannelCommandAdmissionError::Unavailable)
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MutationError {
    #[error(transparent)]
    Core(#[from] ChannelCommandAdmissionError),
    #[error(transparent)]
    Driver(#[from] mongodb::error::Error),
}

impl From<ApplyError> for MutationError {
    fn from(value: ApplyError) -> Self {
        match value {
            ApplyError::Domain(error) => Self::Core(error.into()),
            ApplyError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<super::MutationError> for MutationError {
    fn from(value: super::MutationError) -> Self {
        match value {
            super::MutationError::Invoke(error) => Self::Core(error.into()),
            super::MutationError::Mongo(error) => Self::Driver(error),
        }
    }
}
