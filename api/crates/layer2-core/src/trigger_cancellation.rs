//! Durable release of a cancelled Trigger session's first-turn reservation.

use sha2::{Digest, Sha256};

use crate::event::{Event, EventKind};
use crate::service::SessionService;
use crate::store::{InitialPromptDispatch, SessionRecord, StoreError};
use crate::trigger::{
    Launch, LaunchState, PromptDelivery, PromptDeliveryState, TriggerSessionCancellation,
    TriggerStoreError,
};
use crate::SessionId;

#[async_trait::async_trait]
impl TriggerSessionCancellation for SessionService {
    async fn settle_cancelled_initial_prompt(
        &self,
        launch: &Launch,
    ) -> Result<PromptDelivery, TriggerStoreError> {
        self.settle_cancelled_initial_prompt_inner(launch).await
    }
}

impl SessionService {
    async fn settle_cancelled_initial_prompt_inner(
        &self,
        launch: &Launch,
    ) -> Result<PromptDelivery, TriggerStoreError> {
        if launch.state != LaunchState::Cancelled {
            return Err(malformed(
                "initial-prompt settlement requires a durably cancelled Launch",
            ));
        }
        let session = launch.session.as_ref().ok_or_else(|| {
            malformed("a cancelled Trigger session settlement has no session link")
        })?;
        let cancelled_at = launch
            .terminal_at
            .as_deref()
            .ok_or_else(|| malformed("a cancelled Trigger Launch has no terminal timestamp"))?;
        if crate::seconds_from_rfc3339(cancelled_at).is_none() {
            return Err(malformed(
                "a cancelled Trigger Launch has a malformed terminal timestamp",
            ));
        }

        let id = session.session_id.clone();
        let _prompt_gate = self.prompt_gates.gate(&id).lock_owned().await;
        let mut record = self
            .store
            .get(&id)
            .await
            .map_err(session_store_unavailable)?
            .ok_or_else(|| malformed("the cancelled Launch session no longer exists"))?;
        validate_session_authority(&record, launch, &id)?;

        let expected = record
            .initial_prompt
            .as_ref()
            .map(|initial| initial.delivery.state);
        if let Some(initial) = record.initial_prompt.as_ref() {
            validate_prompt_authority(initial, launch, &id)?;
        }

        if matches!(
            expected,
            Some(PromptDeliveryState::Completed | PromptDeliveryState::Failed)
        ) {
            self.install_cancelled_prompt_record(&record, expected)
                .await?;
            return Ok(record
                .initial_prompt
                .expect("a terminal state came from a retained prompt")
                .delivery);
        }
        if !matches!(expected, None | Some(PromptDeliveryState::Queued)) {
            return Err(malformed(
                "initial-prompt delivery advanced beyond the cancellation-safe queue boundary",
            ));
        }
        self.require_live_prompt_state(&record, expected).await?;

        let mut initial = match record.initial_prompt.clone() {
            Some(initial) => initial,
            None => cancellation_tombstone(launch, &id, cancelled_at)?,
        };
        initial.delivery.state = PromptDeliveryState::Failed;
        initial.delivery.dispatch_fence_sequence = None;
        if crate::seconds_from_rfc3339(&initial.delivery.queued_at)
            <= crate::seconds_from_rfc3339(cancelled_at)
        {
            initial.delivery.updated_at = cancelled_at.to_string();
        } else {
            initial
                .delivery
                .updated_at
                .clone_from(&initial.delivery.queued_at);
        }

        let event = Event {
            session_id: id.clone(),
            seq: 0,
            kind: EventKind::InitialPromptDeliveryChanged {
                state: PromptDeliveryState::Failed,
            },
            appended_at: String::new(),
        };
        self.persist_initial_prompt_transition_durably(&mut record, expected, initial, event)
            .await
            .map_err(session_store_unavailable)?;
        self.install_cancelled_prompt_record(&record, Some(PromptDeliveryState::Failed))
            .await?;
        Ok(record
            .initial_prompt
            .expect("the durable transition installed a cancellation tombstone")
            .delivery)
    }

    async fn require_live_prompt_state(
        &self,
        record: &SessionRecord,
        expected: Option<PromptDeliveryState>,
    ) -> Result<(), TriggerStoreError> {
        let entries = self.entries.read().await;
        let Some(entry) = entries.get(&record.session.id) else {
            return Ok(());
        };
        if entry.session.owner != record.session.owner
            || entry.trigger_origin != record.trigger_origin
            || entry
                .initial_prompt
                .as_ref()
                .map(|initial| initial.delivery.state)
                != expected
        {
            return Err(malformed(
                "the live Trigger session changed across cancellation settlement",
            ));
        }
        Ok(())
    }

    async fn install_cancelled_prompt_record(
        &self,
        record: &SessionRecord,
        expected: Option<PromptDeliveryState>,
    ) -> Result<(), TriggerStoreError> {
        let mut entries = self.entries.write().await;
        let Some(entry) = entries.get_mut(&record.session.id) else {
            return Ok(());
        };
        if entry.session.owner != record.session.owner
            || entry.trigger_origin != record.trigger_origin
        {
            return Err(malformed(
                "the live Trigger session authority changed after cancellation settlement",
            ));
        }
        let durable_state = record
            .initial_prompt
            .as_ref()
            .map(|initial| initial.delivery.state);
        if durable_state != expected {
            return Err(malformed(
                "the committed Trigger prompt state differs from cancellation settlement",
            ));
        }
        entry.initial_prompt.clone_from(&record.initial_prompt);
        entry.session.last_seq = record.session.last_seq;
        entry
            .event_log
            .synchronize_tail(&mut entry.session.last_seq);
        Ok(())
    }
}

fn validate_session_authority(
    record: &SessionRecord,
    launch: &Launch,
    id: &SessionId,
) -> Result<(), TriggerStoreError> {
    let origin = record
        .trigger_origin
        .as_ref()
        .ok_or_else(|| malformed("the Launch session lost its Trigger origin"))?;
    let origin_session = origin
        .session_id()
        .map_err(|_| malformed("the Launch session origin is malformed"))?;
    if record.session.id != *id
        || origin_session != *id
        || record.session.owner != launch.owner
        || origin.launch_id != launch.id
        || origin.revision_digest != launch.revision_digest
    {
        return Err(malformed(
            "the Launch session no longer matches its cancellation authority",
        ));
    }
    Ok(())
}

fn validate_prompt_authority(
    initial: &InitialPromptDispatch,
    launch: &Launch,
    id: &SessionId,
) -> Result<(), TriggerStoreError> {
    let delivery = &initial.delivery;
    if delivery.launch_id != launch.id
        || delivery.session_id != *id
        || delivery.revision_digest != launch.revision_digest
        || delivery.origin_key
            != format!(
                "cma-trigger:{}:{}:initial",
                launch.id.0, launch.revision_digest
            )
        || delivery.payload_hash != sha256(initial.payload.as_bytes())
    {
        return Err(malformed(
            "the retained initial prompt does not match its cancelled Launch",
        ));
    }
    Ok(())
}

fn cancellation_tombstone(
    launch: &Launch,
    id: &SessionId,
    cancelled_at: &str,
) -> Result<InitialPromptDispatch, TriggerStoreError> {
    let payload = crate::trigger_prompt::render_payload(launch)
        .map_err(|_| malformed("the cancelled Launch prompt cannot be rendered safely"))?;
    Ok(InitialPromptDispatch {
        delivery: PromptDelivery {
            launch_id: launch.id.clone(),
            session_id: id.clone(),
            revision_digest: launch.revision_digest.clone(),
            origin_key: format!(
                "cma-trigger:{}:{}:initial",
                launch.id.0, launch.revision_digest
            ),
            payload_hash: sha256(payload.as_bytes()),
            // `Failed` is the existing terminal, owner-input-releasing state.
            // Attempt zero and no dispatch fence make this a non-dispatchable
            // cancellation tombstone rather than false evidence of a write.
            state: PromptDeliveryState::Failed,
            attempt: 0,
            recovery_generation: 0,
            dispatch_fence_sequence: None,
            queued_at: cancelled_at.to_string(),
            updated_at: cancelled_at.to_string(),
        },
        payload,
        recorded_recovery_generation: 0,
    })
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn malformed(detail: &str) -> TriggerStoreError {
    TriggerStoreError::Malformed(detail.into())
}

fn session_store_unavailable(error: StoreError) -> TriggerStoreError {
    TriggerStoreError::Unavailable(format!(
        "Trigger session cancellation settlement failed: {error}"
    ))
}
