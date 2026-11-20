use std::sync::{Arc, Weak};
use std::time::Duration;

use layer2_core::sandbox::ingress_ownership::{IngressWorkScope, OwnedWork, RecordingSession};
#[cfg(test)]
use layer2_core::sandbox::AgentMessageAssembly;
use layer2_core::sandbox::{AgentConversationError, AgentUpdateRecorder};
use layer2_core::AgentId;
use layer3_adapter::ConversationUpdate;
use tokio::sync::{mpsc, oneshot};
use tokio::task::AbortHandle;

use super::{LiveAgentAttachmentIdentity, LiveAgentRegistry};

const UPDATE_RETRY_DELAY: Duration = Duration::from_secs(1);

enum PendingReceipt {
    Legacy(layer3_adapter::RuntimeOutputReceipt),
    Shared(layer3_adapter::RuntimeOutputUpdate),
}

impl PendingReceipt {
    fn from_update(update: ConversationUpdate) -> Option<Self> {
        match update {
            ConversationUpdate::RetainedEffect(delivery) if delivery.is_shared() => {
                delivery.delivery().receipt?;
                // The receipt's strings belong to the original envelope. Keep
                // that owner through quiet flush instead of making an uncharged copy.
                Some(Self::Shared(delivery))
            }
            update => update
                .delivery()
                .receipt
                .map(|receipt| Self::Legacy(receipt.clone())),
        }
    }
}

impl std::ops::Deref for PendingReceipt {
    type Target = layer3_adapter::RuntimeOutputReceipt;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Legacy(receipt) => receipt,
            Self::Shared(delivery) => delivery
                .delivery()
                .receipt
                .expect("pending deliveries contain a receipt"),
        }
    }
}

#[cfg(test)]
#[path = "updates_ack_ingress_tests.rs"]
mod ack_ingress_tests;
#[cfg(test)]
#[path = "updates_coalesce_tests.rs"]
mod coalesce_tests;
#[cfg(test)]
#[path = "updates_ingress_tests.rs"]
mod ingress_tests;
#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_update_task(
    registry: Weak<LiveAgentRegistry>,
    activated: oneshot::Receiver<LiveAgentAttachmentIdentity>,
    mut updates: mpsc::Receiver<ConversationUpdate>,
    recorder: Arc<dyn AgentUpdateRecorder>,
) -> AbortHandle {
    let authority = layer2_core::agent_ownership::execution_authority();
    let cleanup = registry.clone();
    let task = tokio::spawn(async move {
        let recording = async move {
            let Ok(attachment) = activated.await else {
                return;
            };
            let mut assembly = recorder.recording_session();
            if restore_assembly(
                recorder.as_ref(),
                &attachment.agent_id,
                attachment.runtime_generation,
                &mut assembly,
            )
            .await
            .is_err()
            {
                if let Some(registry) = registry.upgrade() {
                    registry.connection_ended(attachment).await;
                }
                return;
            }
            let _budget_monitor =
                super::budget::monitor(registry.clone(), attachment.clone(), recorder.clone());
            let mut pending = None;
            loop {
                let update = match assembly.assembly().output_flush_deadline() {
                    Some(deadline) => tokio::select! {
                        biased;
                        _ = tokio::time::sleep_until(deadline) => {
                            if flush_output(recorder.as_ref(), &attachment.agent_id, attachment.runtime_generation, &mut assembly, &mut pending).await.is_err() { break; }
                            continue;
                        }
                        update = updates.recv() => update,
                    },
                    None => updates.recv().await,
                };
                let Some(update) = update else {
                    // Closing a connection leaves uncommitted receipts for replay; never ACK on drop.
                    break;
                };
                if update.delivery().receipt.is_none()
                    && pending.is_some()
                    && flush_output(
                        recorder.as_ref(),
                        &attachment.agent_id,
                        attachment.runtime_generation,
                        &mut assembly,
                        &mut pending,
                    )
                    .await
                    .is_err()
                {
                    break;
                }
                if process_update(
                    recorder.as_ref(),
                    &attachment.agent_id,
                    attachment.runtime_generation,
                    &mut assembly,
                    update,
                    &mut pending,
                )
                .await
                .is_err()
                {
                    break;
                }
            }
            if let Some(registry) = registry.upgrade() {
                registry.connection_ended(attachment).await;
            }
        };
        match authority {
            Some(authority) => {
                let _ = authority.run(recording).await;
                if let Some(registry) = cleanup.upgrade() {
                    registry.detach_owner(&authority.token);
                }
            }
            None => recording.await,
        }
    });
    task.abort_handle()
}

async fn restore_assembly(
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    runtime_generation: u64,
    assembly: &mut RecordingSession,
) -> Result<(), OwnedWork<AgentConversationError>> {
    loop {
        match recorder
            .restore_recording(agent_id, runtime_generation, assembly)
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) if matches!(&*error, AgentConversationError::Unavailable(_)) => {
                tracing::warn!(agent_id = %agent_id, error = %*error, "agent checkpoint restoration is unavailable");
                drop(error);
                tokio::time::sleep(UPDATE_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::error!(agent_id = %agent_id, error = %*error, "agent checkpoint restoration stopped");
                return Err(error);
            }
        }
    }
}

async fn process_update(
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    runtime_generation: u64,
    assembly: &mut RecordingSession,
    update: ConversationUpdate,
    pending: &mut Option<PendingReceipt>,
) -> Result<(), OwnedWork<AgentConversationError>> {
    let receipt = update.delivery().receipt;
    loop {
        let attempt = recorder.record_output_in_session(
            agent_id,
            runtime_generation,
            assembly,
            &update,
            std::time::SystemTime::now(),
        );
        let result = if receipt.is_some() {
            observe_recording(attempt).await
        } else {
            attempt.await
        };
        match result {
            Ok(result) => {
                let outcome = &result.outcome;
                if outcome.appended.iter().any(|entry| matches!(&entry.kind,
                    layer2_core::sandbox::AgentLogKind::TurnFailed { failure, .. }
                        if failure.class == layer2_core::sandbox::store::AGENT_LOG_CAPACITY_FAILURE
                )) {
                    // The task is durably failed, but its refused output has not
                    // been recorded. Leave the native journal receipt unacked.
                    return Err(IngressWorkScope::inherit_owned(async {
                        AgentConversationError::CapacityExceeded
                    }).await);
                }
                if let Some(source) = &result.committed {
                    acknowledge(source, receipt, pending, assembly.policy().is_some()).await?;
                    metrics::counter!("cma_agent_output_receipts_total", "outcome" => "committed")
                        .increment(1);
                } else if let Some(receipt) = receipt {
                    metrics::counter!("cma_agent_output_receipts_total", "outcome" => "buffered")
                        .increment(1);
                    if pending
                        .as_ref()
                        .is_none_or(|last| last.source.cursor < receipt.source.cursor)
                    {
                        *pending = PendingReceipt::from_update(update);
                    }
                }
                return Ok(());
            }
            Err(error) if matches!(&*error, AgentConversationError::Unavailable(_)) => {
                tracing::warn!(agent_id = %agent_id, error = %*error, "agent update persistence is unavailable");
                drop(error);
                tokio::time::sleep(UPDATE_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::error!(agent_id = %agent_id, error = %*error, "agent update recorder stopped");
                return Err(error);
            }
        }
    }
}

async fn acknowledge(
    source: &layer3_adapter::RuntimeOutputSource,
    current: Option<&layer3_adapter::RuntimeOutputReceipt>,
    pending: &mut Option<PendingReceipt>,
    admitted: bool,
) -> Result<(), OwnedWork<AgentConversationError>> {
    IngressWorkScope::inherit_owned(async {
        let receipt = current
            .filter(|receipt| &receipt.source == source)
            .or_else(|| {
                pending
                    .as_deref()
                    .filter(|receipt| &receipt.source == source)
            })
            .ok_or(AgentConversationError::InvalidState(
                "committed cursor has no matching delivery receipt",
            ))?;
        if admitted {
            receipt.acknowledge_admitted().map_err(|error| {
                tracing::warn!(error = %error, "admitted output acknowledgement refused");
                AgentConversationError::Unavailable(String::new())
            })?;
        } else {
            receipt
                .acknowledge()
                .await
                .map_err(|error| AgentConversationError::Unavailable(error.to_string()))?;
        }
        if pending.as_ref().is_some_and(|receipt| {
            receipt.source.stream_id == source.stream_id && receipt.source.cursor <= source.cursor
        }) {
            *pending = None;
        }
        Ok(())
    })
    .await
    .transpose()
    .map(drop)
}

async fn flush_output(
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    runtime_generation: u64,
    assembly: &mut RecordingSession,
    pending: &mut Option<PendingReceipt>,
) -> Result<(), OwnedWork<AgentConversationError>> {
    loop {
        match observe_recording(recorder.flush_recording(
            agent_id,
            runtime_generation,
            assembly,
            std::time::SystemTime::now(),
        ))
        .await
        {
            Ok(result) => {
                return match &*result {
                    Some(source) => {
                        acknowledge(source, None, pending, assembly.policy().is_some()).await
                    }
                    None => Ok(()),
                }
            }
            Err(error) if matches!(&*error, AgentConversationError::Unavailable(_)) => {
                tracing::warn!("quiet output checkpoint unavailable; retrying the same batch");
                drop(error);
                tokio::time::sleep(UPDATE_RETRY_DELAY).await;
            }
            Err(error) => {
                tracing::error!("quiet output checkpoint refused; ending attachment");
                return Err(error);
            }
        }
    }
}

async fn observe_recording<F, T>(
    future: F,
) -> Result<OwnedWork<T>, OwnedWork<AgentConversationError>>
where
    F: std::future::Future<Output = Result<OwnedWork<T>, OwnedWork<AgentConversationError>>>,
{
    let mut failure = None;
    let observed = crate::telemetry::agent_output::observe(async {
        match future.await {
            Ok(value) => Ok(value),
            Err(error) => {
                // Telemetry consumes only the category. Keep the real diagnostic
                // with its owner instead of cloning it out of admitted storage.
                let category = match &*error {
                    AgentConversationError::Unavailable(_) => {
                        AgentConversationError::Unavailable(String::new())
                    }
                    AgentConversationError::StaleGeneration => {
                        AgentConversationError::StaleGeneration
                    }
                    AgentConversationError::CapacityExceeded => {
                        AgentConversationError::CapacityExceeded
                    }
                    _ => AgentConversationError::InvalidState("recording refused"),
                };
                failure = Some(error);
                Err(category)
            }
        }
    })
    .await;
    observed.map_err(|_| failure.expect("observed failure retains its original owner"))
}

#[cfg(test)]
pub(crate) async fn record_update(
    recorder: &dyn AgentUpdateRecorder,
    agent_id: &AgentId,
    runtime_generation: u64,
    assembly: &mut AgentMessageAssembly,
    update: ConversationUpdate,
) -> Result<(), AgentConversationError> {
    // Existing capacity fixtures own the legacy DTO directly. Restore it even
    // if they cancel this helper while recording is suspended.
    struct LegacyAssembly<'a> {
        session: RecordingSession,
        destination: &'a mut AgentMessageAssembly,
    }
    impl Drop for LegacyAssembly<'_> {
        fn drop(&mut self) {
            *self.destination =
                std::mem::take(self.session.legacy_assembly_mut().expect("legacy fixture"));
        }
    }
    let mut session = RecordingSession::new(None);
    *session.legacy_assembly_mut().expect("legacy fixture") = std::mem::take(assembly);
    let mut legacy = LegacyAssembly {
        session,
        destination: assembly,
    };
    process_update(
        recorder,
        agent_id,
        runtime_generation,
        &mut legacy.session,
        update,
        &mut None,
    )
    .await
    .map_err(|error| {
        error
            .into_current()
            .expect("legacy fixture retains the receiving scope")
    })
}
