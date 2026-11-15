//! Closed store-only polling. No callback, provider handle or caller query can
//! enter this transaction. Its receipt never settles a logical effect permit.
mod model;
mod records;
mod transaction;

pub(crate) use model::{
    NativePollCommand, NativePollExecution, NativePollLane, NativePollOutcome, NativePollReceipt,
};

use super::{
    AuthorityError, AuthorityStore, Phase, ReleaseHead, ReleaseTarget, Result, WriterIdentity,
};
use model::{command_key, CommandRecord};

#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;

impl AuthorityStore {
    /// A fixed producer advances only after observing its exact durable tip.
    /// Save the returned command before execute. A pending command must be
    /// reconciled, even if reconciliation currently finds no receipt.
    pub(crate) async fn prepare_native_poll(
        &self,
        writer: &WriterIdentity,
        lane: NativePollLane,
        observed_previous: Option<&NativePollReceipt>,
    ) -> Result<NativePollCommand> {
        writer.validate()?;
        let mut session = self.transaction().await?;
        let result = async {
            let head = self.head_in(&writer.deployment, &mut session).await?;
            active_writer(&head, writer)?;
            let tip = records::tip(self, writer, lane, &mut session).await?;
            let sequence = records::next_sequence(tip.as_ref(), observed_previous)?;
            let command = CommandRecord {
                key: command_key(writer, lane, sequence)?,
                writer: writer.clone(),
                lane,
                sequence,
                previous_receipt_sha256: tip.map(|r| r.0.receipt_sha256),
                selected_target: selected(&head, lane)?,
            };
            command.validate()?;
            Ok(NativePollCommand(command))
        }
        .await;
        // Read-only snapshot; no commit can create an admission here.
        let _ = session.abort_transaction().await;
        result
    }

    /// A Fresh result is returned only after this invocation's commit ACK.
    /// Commit uncertainty never retries the transaction or changes sequence.
    pub(crate) async fn execute_native_poll(
        &self,
        command: &NativePollCommand,
    ) -> Result<NativePollExecution> {
        if let Err(error) = command.0.validate() {
            return Ok(NativePollExecution::NotCommitted(error));
        }
        let session = match self.transaction().await {
            Ok(session) => session,
            Err(error) => return Ok(NativePollExecution::NotCommitted(error)),
        };
        Ok(self.execute_native_poll_in(command, session).await)
    }

    // Closed implementation seam, also used by the controlled snapshot-conflict
    // fixture. No caller callback/provider action or external session is exposed.
    async fn execute_native_poll_in(
        &self,
        command: &NativePollCommand,
        mut session: mongodb::ClientSession,
    ) -> NativePollExecution {
        let result = transaction::execute(self, &command.0, &mut session).await;
        match result {
            Ok(NativePollExecution::Fresh(receipt)) => match session.commit_transaction().await {
                Ok(()) => NativePollExecution::Fresh(receipt),
                // Once commit was invoked, no error is a retry certificate.
                Err(_) => NativePollExecution::Uncertain,
            },
            Ok(other) => {
                let _ = session.abort_transaction().await;
                other
            }
            Err(error) => {
                // All body writes belong to this transaction. Commit was never
                // requested, even if abort's ACK is lost. No provider was called.
                let _ = session.abort_transaction().await;
                NativePollExecution::NotCommitted(error)
            }
        }
    }

    /// Historical result only, including after freeze. None never authorizes
    /// retransmission: the original transaction may still be completing.
    pub(crate) async fn reconcile_native_poll(
        &self,
        command: &NativePollCommand,
    ) -> Result<Option<NativePollReceipt>> {
        command.0.validate()?;
        let mut session = self.transaction().await?;
        let result = records::receipt(self, &command.0, &mut session).await;
        let _ = session.abort_transaction().await;
        result
    }

    /// Reconciliation inventory, not permission to skip a pending command or
    /// dispatch a returned positive lease. A caller must retain that distinction.
    pub(crate) async fn native_poll_tip(
        &self,
        writer: &WriterIdentity,
        lane: NativePollLane,
    ) -> Result<Option<NativePollReceipt>> {
        writer.validate()?;
        let mut session = self.transaction().await?;
        let result = records::tip(self, writer, lane, &mut session).await;
        let _ = session.abort_transaction().await;
        result
    }
}

fn active_writer(head: &ReleaseHead, writer: &WriterIdentity) -> Result<()> {
    head.validate()?;
    if head.phase != Phase::Active || head.writer.as_ref() != Some(writer) {
        return Err(AuthorityError::Closed);
    }
    Ok(())
}

fn selected(head: &ReleaseHead, lane: NativePollLane) -> Result<Option<ReleaseTarget>> {
    match lane {
        NativePollLane::Background => Ok(None),
        NativePollLane::StartupAgent => Ok(Some(head.pair.agent.clone())),
        NativePollLane::StartupMachine => head
            .pair
            .machine
            .clone()
            .map(Some)
            .ok_or(AuthorityError::Closed),
    }
}
