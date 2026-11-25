//! Versioned sidecars are opened explicitly by activation-aware composition.
//! Legacy store construction does not create collections or execution records.

use crate::store::transaction_terminal::GuardedTerminal as _;
mod collections;
#[cfg(test)]
mod compat_tests;
mod effects;
mod leases;
#[cfg(test)]
mod mongo_tests;
mod records;
mod revocation;
pub(super) use collections::refuse_legacy_change;
pub(super) use revocation::FinalizationPermit;

use layer2_core::resource_access::execution::*;
use layer3_adapter::resource_execution_protocol as wire;
use mongodb::{
    bson::{doc, Document},
    ClientSession,
};
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;

use super::super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, start_transaction, CommitOutcome,
    MongoSandboxStore, TRANSACTION_ATTEMPTS,
};
use collections::Collections;

#[derive(Clone)]
pub struct MongoResourceExecutionStore {
    sandbox: MongoSandboxStore,
    collections: Collections,
}

enum TxnError {
    Domain(ExecutionError),
    Driver(mongodb::error::Error),
}
impl std::fmt::Debug for TxnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Domain(error) => error.fmt(f),
            Self::Driver(_) => f.write_str("resource execution database error"),
        }
    }
}
type TxnResult<T> = Result<T, TxnError>;
impl From<ExecutionError> for TxnError {
    fn from(value: ExecutionError) -> Self {
        Self::Domain(value)
    }
}
impl From<wire::ProtocolError> for TxnError {
    fn from(value: wire::ProtocolError) -> Self {
        Self::Domain(value.into())
    }
}
impl From<mongodb::error::Error> for TxnError {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<super::ReadError> for TxnError {
    fn from(value: super::ReadError) -> Self {
        match value {
            super::ReadError::Domain(error) => Self::Domain(error.into()),
            super::ReadError::Driver(error) => Self::Driver(error),
        }
    }
}

enum Command<'a> {
    Bootstrap,
    Activate(wire::Generation, &'a wire::Digest),
    Register(&'a ExecutionAuthority, &'a wire::MemberKey),
    Ticket(&'a ExecutionAuthority, &'a wire::ExecutionTicketV1),
    Continue(&'a ExecutionAuthority, &'a wire::ContinuationV1),
    Release(&'a ExecutionAuthority, &'a wire::ReleasePermitV1),
    Start(&'a wire::StartObservationV1),
    Freeze(&'a RevocationAuthority),
    Barrier(&'a wire::BarrierReceiptV1),
    Completion(&'a MemberCompletionV1),
    Finalize(&'a wire::Digest),
    Effect(&'a wire::EffectIntentV1),
    Outcome(&'a wire::EffectObservationV1),
}
enum Output {
    Record(Box<ExecutionRecordV1>),
    Ticket(wire::ExecutionTicketV1),
    Continue(wire::ContinuationV1),
    Release(wire::ReleasePermitV1),
    Revocation(RevocationOperationV1),
    Effect(wire::EffectPermissionV1),
    Observed,
}

impl MongoResourceExecutionStore {
    async fn transact(&self, command: Command<'_>) -> ExecutionResult<Output> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .sandbox
                .client
                .start_session()
                .await
                .map_err(|_| ExecutionError::Unavailable)?;
            start_transaction(&mut session)
                .await
                .map_err(|_| ExecutionError::Unavailable)?;
            let result = self.apply(&command, &mut session).await;
            let output = match result {
                Ok(output) => output,
                Err(error) => {
                    if session.abort_guarded().await.is_err() {
                        tracing::warn!("resource execution transaction abort was not acknowledged");
                    }
                    match error {
                        TxnError::Domain(error) => return Err(error),
                        TxnError::Driver(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        TxnError::Driver(_) => return Err(ExecutionError::Unavailable),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(output),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Unknown(_) => {
                    tracing::warn!(
                        "resource execution commit remains unknown; reconcile original identity"
                    );
                    return Err(ExecutionError::Unknown);
                }
                CommitOutcome::Failed(_) => return Err(ExecutionError::Unavailable),
            }
        }
        Err(ExecutionError::Unavailable)
    }

    async fn apply(&self, command: &Command<'_>, session: &mut ClientSession) -> TxnResult<Output> {
        if !matches!(command, Command::Bootstrap | Command::Activate(_, _)) {
            self.collections.require_activation(session).await?;
        }
        match command {
            Command::Bootstrap => self
                .collections
                .initialize(session)
                .await
                .map(|()| Output::Observed),
            Command::Activate(epoch, evidence) => self
                .collections
                .activate(*epoch, evidence, session)
                .await
                .map(|()| Output::Observed),
            Command::Register(authority, member) => self
                .register_member(authority, member, session)
                .await
                .map(|record| Output::Record(Box::new(record))),
            Command::Ticket(authority, ticket) => self
                .persist_ticket(authority, ticket, session)
                .await
                .map(Output::Ticket),
            Command::Continue(authority, lease) => self
                .persist_continuation(authority, lease, session)
                .await
                .map(Output::Continue),
            Command::Release(authority, permit) => self
                .persist_release(authority, permit, session)
                .await
                .map(Output::Release),
            Command::Start(start) => self
                .persist_start(start, session)
                .await
                .map(|()| Output::Observed),
            Command::Freeze(authority) => self
                .freeze(authority, session)
                .await
                .map(Output::Revocation),
            Command::Barrier(receipt) => self
                .persist_barrier(receipt, session)
                .await
                .map(|()| Output::Observed),
            Command::Completion(receipt) => self
                .persist_completion(receipt, session)
                .await
                .map(|()| Output::Observed),
            Command::Finalize(operation) => self
                .finalize(operation, session)
                .await
                .map(Output::Revocation),
            Command::Effect(intent) => self
                .persist_effect(intent, session)
                .await
                .map(Output::Effect),
            Command::Outcome(observation) => self
                .persist_outcome(observation, session)
                .await
                .map(|()| Output::Observed),
        }
    }
}

#[async_trait::async_trait]
impl ResourceExecutionStore for MongoResourceExecutionStore {
    async fn activation(&self) -> ExecutionResult<ActivationStateV1> {
        self.collections.activation().await
    }
    async fn grant(&self, origin: &wire::Digest) -> ExecutionResult<Option<GrantControlV1>> {
        self.collections.grant(origin).await
    }
    async fn record(
        &self,
        registration: &wire::Digest,
    ) -> ExecutionResult<Option<ExecutionRecordV1>> {
        let value = self
            .collections
            .members
            .find_one(doc! {"_id":registration.as_str()})
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        value.as_ref().map(records::read_member).transpose()
    }
    async fn revocation(
        &self,
        operation: &wire::Digest,
    ) -> ExecutionResult<Option<RevocationOperationV1>> {
        let value = self
            .collections
            .operations
            .find_one(doc! {"_id":operation.as_str()})
            .await
            .map_err(|_| ExecutionError::Unavailable)?;
        value.as_ref().map(records::read_operation).transpose()
    }
    async fn register(
        &self,
        authority: &ExecutionAuthority,
        member: &wire::MemberKey,
    ) -> ExecutionResult<ExecutionRecordV1> {
        match self.transact(Command::Register(authority, member)).await? {
            Output::Record(value) => Ok(*value),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn issue_ticket(
        &self,
        authority: &ExecutionAuthority,
        ticket: &wire::ExecutionTicketV1,
    ) -> ExecutionResult<wire::ExecutionTicketV1> {
        match self.transact(Command::Ticket(authority, ticket)).await? {
            Output::Ticket(value) => Ok(value),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn continue_execution(
        &self,
        authority: &ExecutionAuthority,
        lease: &wire::ContinuationV1,
    ) -> ExecutionResult<wire::ContinuationV1> {
        match self.transact(Command::Continue(authority, lease)).await? {
            Output::Continue(value) => Ok(value),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn issue_release(
        &self,
        authority: &ExecutionAuthority,
        permit: &wire::ReleasePermitV1,
    ) -> ExecutionResult<wire::ReleasePermitV1> {
        match self.transact(Command::Release(authority, permit)).await? {
            Output::Release(value) => Ok(value),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn observe_start(&self, observation: &wire::StartObservationV1) -> ExecutionResult<()> {
        match self.transact(Command::Start(observation)).await? {
            Output::Observed => Ok(()),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn begin_revocation(
        &self,
        authority: &RevocationAuthority,
    ) -> ExecutionResult<RevocationOperationV1> {
        match self.transact(Command::Freeze(authority)).await? {
            Output::Revocation(value) => Ok(value),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn observe_barrier(&self, receipt: &wire::BarrierReceiptV1) -> ExecutionResult<()> {
        match self.transact(Command::Barrier(receipt)).await? {
            Output::Observed => Ok(()),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn observe_completion(&self, completion: &MemberCompletionV1) -> ExecutionResult<()> {
        match self.transact(Command::Completion(completion)).await? {
            Output::Observed => Ok(()),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn finish_revocation(
        &self,
        operation: &wire::Digest,
    ) -> ExecutionResult<RevocationOperationV1> {
        match self.transact(Command::Finalize(operation)).await? {
            Output::Revocation(value) => Ok(value),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn prepare_effect(
        &self,
        intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<wire::EffectPermissionV1> {
        match self.transact(Command::Effect(intent)).await? {
            Output::Effect(value) => Ok(value),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn observe_effect(&self, observation: &wire::EffectObservationV1) -> ExecutionResult<()> {
        match self.transact(Command::Outcome(observation)).await? {
            Output::Observed => Ok(()),
            _ => Err(ExecutionError::Corrupt),
        }
    }
    async fn pending_revocations(
        &self,
        after: Option<&wire::Digest>,
        limit: u32,
    ) -> ExecutionResult<Vec<RevocationOperationV1>> {
        self.pending_page(after, limit).await
    }
    async fn frozen_members(
        &self,
        operation: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<ExecutionRecordV1>> {
        self.member_page(operation, after, limit).await
    }
    async fn effects_for(
        &self,
        registration: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<EffectRecordV1>> {
        self.effect_page(registration, after, limit).await
    }
}
