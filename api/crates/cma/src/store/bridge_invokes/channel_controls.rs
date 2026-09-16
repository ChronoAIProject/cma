//! Atomic signed control intent, shared v2 receipt and held common command.
//! Fresh production admission remains unavailable until runtime routing is proved.
use layer2_core::bridge::channel_control::*;
use layer2_core::sandbox::agent_io::CommandReservationResult;
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection,
};
use std::sync::Arc;

use super::MongoBridgeInvokeStore;
use crate::store::{
    agent_commands::{read_command, reserve, ApplyError},
    durability::majority_collection,
    sandbox_mongo::{
        commit_transaction, is_duplicate_key, is_transient, record_bson, start_transaction,
        CommitOutcome, DOCUMENT_SCHEMA_VERSION,
    },
};

mod acceptance;
mod fence;
mod observation;
pub(crate) use acceptance::ChannelControlAcceptance;
pub(in crate::store) mod records;
use records::{read_admitted, read_intent};
type Error = ChannelControlError;

pub(crate) struct MongoChannelControlStore {
    invokes: Arc<MongoBridgeInvokeStore>,
    intents: Collection<Document>,
    aliases: Collection<Document>,
    callbacks: Arc<crate::store::MongoCallbackInbox>,
}

impl MongoChannelControlStore {
    pub(crate) async fn open(
        invokes: Arc<MongoBridgeInvokeStore>,
        callbacks: Arc<crate::store::MongoCallbackInbox>,
    ) -> Result<Self, Error> {
        callbacks
            .require_control_store(&invokes.bridges)
            .map_err(|_| Error::Unavailable)?;
        let database = invokes
            .bridges
            .client
            .database(&invokes.bridges.definitions.namespace().db);
        crate::store::schema::initialize_collections(
            &database,
            &[
                "bridge_channel_control_intents",
                "bridge_channel_control_aliases",
            ],
        )
        .await
        .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            invokes,
            callbacks,
            intents: majority_collection(&database, "bridge_channel_control_intents"),
            aliases: majority_collection(&database, "bridge_channel_control_aliases"),
        })
    }

    pub(crate) async fn reserve(
        &self,
        request: ChannelControlReservation,
    ) -> Result<AdmittedChannelControl, Error> {
        self.admit(request).await?.admitted.ok_or(Error::Conflict)
    }

    pub(crate) async fn recover(
        &self,
        write: &ChannelControlWrite,
    ) -> Result<Option<AdmittedChannelControl>, Error> {
        self.recover_acceptance(write)
            .await?
            .map(|value| value.admitted.ok_or(Error::Conflict))
            .transpose()
    }

    async fn recover_transaction(
        &self,
        write: &ChannelControlRecovery<'_>,
        session: &mut ClientSession,
        allow_fresh_refusal: bool,
    ) -> Result<Option<ChannelControlAcceptance>, MutationError> {
        let receipt = if let Some(receipt) = self
            .callbacks
            .recover_control_disposition(write.inbox(), session)
            .await?
        {
            receipt
        } else {
            let callback = write.callback();
            if self
                .intents
                .find_one(doc! {"_id":callback.intent_id()?})
                .session(&mut *session)
                .await?
                .is_some()
            {
                return Err(Error::Corrupt.into());
            }
            if !allow_fresh_refusal {
                return Ok(None);
            }
            self.callbacks
                .refuse_fresh_control(write.inbox(), session)
                .await?
        };
        self.resolve_acceptance(write, receipt, session)
            .await
            .map(Some)
    }

    pub(super) async fn transaction(
        &self,
        request: &ChannelControlReservation,
        session: &mut ClientSession,
    ) -> Result<ChannelControlAcceptance, MutationError> {
        request.require_current()?;
        let proof = request.proof();
        if let Some(receipt) = self
            .callbacks
            .prepare_control(proof.write().inbox(), session)
            .await?
        {
            let recovery = ChannelControlRecovery::from_authenticated(proof.write().inbox())?;
            return self.resolve_acceptance(&recovery, receipt, session).await;
        }
        self.require_unclaimed_callback(proof.write(), session)
            .await?;
        let candidate = proof.intent();
        if self
            .intents
            .find_one(doc! {"_id":&candidate.id})
            .session(&mut *session)
            .await?
            .is_some()
        {
            // An intent without its shared acceptance is partial/corrupt state,
            // never permission to manufacture an acceptance around old work.
            return Err(Error::Corrupt.into());
        }
        self.fence_source(proof, session).await?;
        let CommandReservationResult::Created(record) =
            reserve::apply(&self.invokes.resources, request.command(), session).await?
        else {
            // A ledger command without this complete permanent intent is not
            // evidence that a new intent may be manufactured around it.
            return Err(Error::Corrupt.into());
        };
        let result = AdmittedChannelControl {
            command: *record,
            sidecar: request.sidecar()?,
        };
        request.require_result(&result)?;
        let changed = self.invokes.resources.agent_commands.update_one(
            doc! {"_id":result.sidecar.command_id.as_str(),"channel_admission":{"$exists":false},"channel_control_admission":{"$exists":false}},
            doc! {"$set":{"channel_control_admission":record_bson(&result.sidecar).map_err(|_| Error::Corrupt)?}},
        ).session(&mut *session).await?;
        if changed.matched_count != 1 {
            return Err(Error::Conflict.into());
        }
        self.intents
            .insert_one(records::intent_document(candidate)?)
            .session(&mut *session)
            .await?;
        let acceptance = self
            .callbacks
            .retain_accepted_control(proof.write(), &result, session)
            .await?;
        self.retain_alias(&candidate.callback, candidate, session)
            .await?;
        let database = self
            .invokes
            .bridges
            .client
            .database(&self.intents.namespace().db);
        use crate::store::channel_history::{control_record, ReferenceKind};
        let command_row = self
            .invokes
            .resources
            .agent_commands
            .find_one(doc! {"_id": result.sidecar.command_id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(Error::Corrupt)?;
        control_record(
            &database,
            &candidate.reference,
            ReferenceKind::ControlCommand,
            &command_row,
            session,
        )
        .await?;
        control_record(
            &database,
            &candidate.reference,
            ReferenceKind::ControlIntent,
            &records::intent_document(candidate)?,
            session,
        )
        .await?;
        request.require_current()?;
        Ok(ChannelControlAcceptance {
            acceptance,
            admitted: Some(result),
        })
    }

    async fn resolve_acceptance(
        &self,
        write: &ChannelControlRecovery<'_>,
        acceptance: layer2_core::bridge::ingress::CallbackAcceptance,
        session: &mut ClientSession,
    ) -> Result<ChannelControlAcceptance, MutationError> {
        use layer2_core::bridge::ingress::CallbackAcceptance;
        let CallbackAcceptance::ControlAccepted(receipt) = &acceptance else {
            return Ok(ChannelControlAcceptance {
                acceptance,
                admitted: None,
            });
        };
        let document = self
            .intents
            .find_one(doc! {"_id":&receipt.intent_id})
            .session(&mut *session)
            .await?
            .ok_or(Error::Corrupt)?;
        let intent = read_intent(&document)?;
        if intent.expected_acceptance()? != *receipt || intent.request != *write.request() {
            return Err(Error::Conflict.into());
        }
        let callback = write.callback();
        intent.callback.require_alias(callback)?;
        let result = self.read_result(&intent, session).await?;
        let database = self
            .invokes
            .bridges
            .client
            .database(&self.intents.namespace().db);
        crate::store::MongoCallbackInbox::fence_accepted_control(&database, &intent, session)
            .await?;
        self.invokes
            .fence_bridge(write.inbox().bridge(), session)
            .await?;
        self.retain_alias(callback, &intent, session).await?;
        Ok(ChannelControlAcceptance {
            acceptance,
            admitted: Some(result),
        })
    }

    async fn read_result(
        &self,
        intent: &ChannelControlIntent,
        session: &mut ClientSession,
    ) -> Result<AdmittedChannelControl, MutationError> {
        let document = self
            .invokes
            .resources
            .agent_commands
            .find_one(doc! {"_id":intent.command_id()?.as_str()})
            .session(session)
            .await?
            .ok_or(Error::Corrupt)?;
        let result = read_admitted(&document)?;
        if result.sidecar.intent != *intent {
            return Err(Error::Corrupt.into());
        }
        Ok(result)
    }

    async fn retain_alias(
        &self,
        callback: &ChannelControlCallback,
        intent: &ChannelControlIntent,
        session: &mut ClientSession,
    ) -> Result<(), MutationError> {
        if let Some(document) = self
            .aliases
            .find_one(doc! {"_id":&callback.callback_id})
            .session(&mut *session)
            .await?
        {
            records::require_alias(&document, callback, intent)?;
        } else {
            self.aliases
                .insert_one(records::alias_document(callback, intent)?)
                .session(&mut *session)
                .await?;
        }
        let database = self
            .invokes
            .bridges
            .client
            .database(&self.intents.namespace().db);
        use crate::store::channel_history::{control_record, ReferenceKind};
        control_record(
            &database,
            &intent.reference,
            ReferenceKind::ControlAlias,
            &records::alias_document(callback, intent)?,
            session,
        )
        .await?;
        for row in crate::store::MongoCallbackInbox::control_history_documents(
            &database, intent, callback, session,
        )
        .await?
        {
            control_record(
                &database,
                &intent.reference,
                ReferenceKind::ControlDisposition,
                &row,
                session,
            )
            .await?;
        }
        Ok(())
    }
}

pub(super) enum MutationError {
    Core(Error),
    Driver(mongodb::error::Error),
}
impl MutationError {
    fn core(self) -> Error {
        match self {
            Self::Core(error) => error,
            Self::Driver(_) => Error::Unavailable,
        }
    }
}
impl From<Error> for MutationError {
    fn from(error: Error) -> Self {
        Self::Core(error)
    }
}
impl From<mongodb::error::Error> for MutationError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Driver(error)
    }
}
impl From<super::MutationError> for MutationError {
    fn from(error: super::MutationError) -> Self {
        match error {
            super::MutationError::Invoke(_) => Self::Core(Error::AuthorityUnavailable),
            super::MutationError::Mongo(error) => Self::Driver(error),
        }
    }
}
impl From<super::channel::MutationError> for MutationError {
    fn from(error: super::channel::MutationError) -> Self {
        match error {
            super::channel::MutationError::Core(_) => Self::Core(Error::SourceUnavailable),
            super::channel::MutationError::Driver(error) => Self::Driver(error),
        }
    }
}
impl From<ApplyError> for MutationError {
    fn from(error: ApplyError) -> Self {
        match error {
            ApplyError::Domain(_) => Self::Core(Error::Conflict),
            ApplyError::Driver(error) => Self::Driver(error),
        }
    }
}

impl From<crate::store::bridge_definitions::inbox::MutationError> for MutationError {
    fn from(error: crate::store::bridge_definitions::inbox::MutationError) -> Self {
        use layer2_core::bridge::ingress::CallbackError;
        match error {
            crate::store::bridge_definitions::inbox::MutationError::Mongo(error) => {
                Self::Driver(error)
            }
            crate::store::bridge_definitions::inbox::MutationError::Core(error) => {
                Self::Core(match error {
                    CallbackError::ReplayConflict | CallbackError::Capacity => Error::Conflict,
                    CallbackError::BindingUnavailable => Error::AuthorityUnavailable,
                    CallbackError::Payload => Error::Invalid,
                    _ => Error::Unavailable,
                })
            }
        }
    }
}

impl From<crate::store::bridge_allocations::MutationError> for MutationError {
    fn from(error: crate::store::bridge_allocations::MutationError) -> Self {
        match error {
            crate::store::bridge_allocations::MutationError::Mongo(error) => Self::Driver(error),
            crate::store::bridge_allocations::MutationError::Core(_) => {
                Self::Core(Error::SourceUnavailable)
            }
        }
    }
}
