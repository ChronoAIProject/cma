//! Default-closed executing coordinator skeleton. No production port exists.
//! Each call performs at most one durable progress step or one original-operation
//! dispatch. The integrating scheduler supplies finite deadlines and fairness
//! between homes; these callbacks never release a home or resurrect an Agent.
#[path = "ledger_collector/owned_window.rs"]
pub mod owned_window;
#[path = "ledger_collector/port.rs"]
pub mod port;
#[path = "ledger_collector/types.rs"]
pub mod types;

use port::{CollectorPort, Held};
use types::*;

#[derive(Debug, Clone, Copy, Default)]
pub struct Admission {
    /// Scheduling admission only. This cannot construct a maintenance permit.
    pub new_work: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Disabled,
    InventoryComplete,
    PagePinned,
    ReferenceRecorded,
    ReferenceRecovered,
    EvidenceSelected,
    OriginalSubmitted,
    ReceiptRecorded,
    GroupSettled,
    PageAdvanced,
    RevisionRefreshed,
}

/// Closing new-work admission must preserve settlement of admitted operations.
/// No effect callback runs when closed and no pending work exists. A load is required
/// to distinguish those cases; load itself is a read with no effect authority.
pub async fn step<P: CollectorPort>(admission: Admission, port: &P) -> Result<Step> {
    let checkpoint = port.load().await?;
    if !checkpoint.valid() {
        return Err(Refusal::InvalidEvidence);
    }
    let retained = discover(port, &checkpoint).await?;
    if !checkpoint.has_work() && retained.is_none() && !admission.new_work {
        return Ok(Step::Disabled);
    }
    let hold = port.acquire(&checkpoint).await?;
    advance(port, &checkpoint, hold, retained).await
}

async fn discover<P: CollectorPort>(
    port: &P,
    checkpoint: &Checkpoint,
) -> Result<Option<CollectionReference>> {
    if checkpoint.has_work() {
        return Ok(None);
    }
    let retained = port.pending_collection(&checkpoint.binding).await?;
    if retained.as_ref().is_some_and(|r| {
        !r.validate()
            || r.binding_sha256 != hex(&checkpoint.binding.sha256)
            || r.archive_id != uuid::Uuid::from_bytes(checkpoint.binding.archive_id).to_string()
    }) {
        return Err(Refusal::InvalidEvidence);
    }
    Ok(retained)
}

/// One state transition under a caller-owned gate. The owned window keeps an
/// additional Arc through the whole admitted group, even across reply loss.
async fn advance<P: CollectorPort>(
    port: &P,
    checkpoint: &Checkpoint,
    hold: std::sync::Arc<P::Hold>,
    retained: Option<CollectionReference>,
) -> Result<Step> {
    if !checkpoint.valid() {
        return Err(Refusal::InvalidEvidence);
    }
    if hold.binding() != checkpoint.binding {
        return Err(Refusal::StaleAuthority);
    }
    if let Some(reference) = retained {
        if checkpoint.has_work() {
            return Err(Refusal::Conflict);
        }
        port.recover_reference(checkpoint, reference).await?;
        return Ok(Step::ReferenceRecovered);
    }
    if let Some(pending) = &checkpoint.pending {
        port.verify_prepared(&pending.intent).await?;
        if pending.intent.decision != Decision::SettledDeleted {
            port.settle(checkpoint).await?;
            return Ok(Step::GroupSettled);
        }
        let reference = pending.reference.as_ref().ok_or(Refusal::InvalidEvidence)?;
        port.read_collection(&pending.intent, reference).await?;
        match port
            .collection_completion(&pending.intent, reference)
            .await?
        {
            Some(receipt) => {
                if !receipt.matches(&pending.intent) {
                    return Err(Refusal::InvalidEvidence);
                }
                if let Some(recorded) = &pending.completion {
                    if *recorded != receipt {
                        return Err(Refusal::InvalidEvidence);
                    }
                    port.settle(checkpoint).await?;
                    Ok(Step::GroupSettled)
                } else {
                    port.record_receipt(checkpoint, receipt).await?;
                    Ok(Step::ReceiptRecorded)
                }
            }
            None => {
                if pending.completion.is_some() {
                    return Err(Refusal::UnknownOutcome);
                }
                let reply = port
                    .complete_collection_held(hold, &pending.intent, reference)
                    .await?;
                if !reply.matches(&pending.intent) {
                    return Err(Refusal::InvalidEvidence);
                }
                // The call's reply is not B's durable receipt: read the cold
                // archive on the next step before recording or advancing.
                Ok(Step::OriginalSubmitted)
            }
        }
    } else if let Some(staged) = &checkpoint.staged {
        port.verify_prepared(&staged.intent).await?;
        if let Some(reference) = &staged.reference {
            port.read_collection(&staged.intent, reference).await?;
        }
        port.select(checkpoint).await?;
        Ok(Step::EvidenceSelected)
    } else {
        if hold.source_revision() != checkpoint.source_revision {
            if hold.source_revision() < checkpoint.source_revision {
                return Err(Refusal::StaleCursor);
            }
            port.refresh_revision(checkpoint, hold.source_revision())
                .await?;
            return Ok(Step::RevisionRefreshed);
        }
        if checkpoint.complete {
            return Ok(Step::InventoryComplete);
        }
        match &checkpoint.page {
            None => {
                let page = port.enumerate(hold.clone(), checkpoint).await?;
                if !page.valid()
                    || page.source_revision != checkpoint.source_revision
                    || page.cursor != checkpoint.cursor
                    || page.groups.first().is_some_and(|first| {
                        checkpoint.after_group.is_some_and(|after| *first <= after)
                    })
                {
                    return Err(Refusal::InvalidEvidence);
                }
                port.pin_page(checkpoint, page).await?;
                Ok(Step::PagePinned)
            }
            Some(page) => match page.groups.first() {
                None => {
                    port.advance_page(checkpoint).await?;
                    Ok(Step::PageAdvanced)
                }
                Some(group) => {
                    let staged = port.observe_and_retain(hold, checkpoint, *group).await?;
                    if !staged.valid()
                        || staged.intent.group != *group
                        || staged.intent.binding != checkpoint.binding
                        || staged.intent.before_revision != checkpoint.source_revision
                    {
                        return Err(Refusal::InvalidEvidence);
                    }
                    port.record_reference(checkpoint, staged).await?;
                    Ok(Step::ReferenceRecorded)
                }
            },
        }
    }
}

#[cfg(test)]
#[path = "ledger_collector/tests.rs"]
mod tests;
