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

use port::{CollectorPort, Held, Reconciled};
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
    EvidenceSelected,
    OriginalSubmitted,
    ReceiptRecorded(Phase),
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
    if checkpoint.pending.is_none() && !admission.new_work {
        return Ok(Step::Disabled);
    }
    let hold = port.acquire(&checkpoint).await?;
    advance(port, &checkpoint, hold).await
}

/// One state transition under a caller-owned gate. The owned window keeps an
/// additional Arc through the whole admitted group, even across reply loss.
async fn advance<P: CollectorPort>(
    port: &P,
    checkpoint: &Checkpoint,
    hold: std::sync::Arc<P::Hold>,
) -> Result<Step> {
    if !checkpoint.valid() {
        return Err(Refusal::InvalidEvidence);
    }
    if hold.binding() != checkpoint.binding {
        return Err(Refusal::StaleAuthority);
    }
    if let Some(pending) = &checkpoint.pending {
        port.verify_retained(&pending.intent).await?;
        if pending.intent.decision != Decision::SettledDeleted {
            port.settle(checkpoint).await?;
            return Ok(Step::GroupSettled);
        }
        let Some(phase) = pending.next_phase() else {
            port.settle(checkpoint).await?;
            return Ok(Step::GroupSettled);
        };
        match port.reconcile(hold.clone(), &pending.intent, phase).await? {
            Reconciled::OriginalReceipt(receipt) => {
                if !receipt.matches(&pending.intent, phase)
                    || (phase == Phase::Acknowledged
                        && pending.receipts[1].after_revision != receipt.after_revision)
                {
                    return Err(Refusal::InvalidEvidence);
                }
                port.record_receipt(checkpoint, *receipt).await?;
                Ok(Step::ReceiptRecorded(phase))
            }
            Reconciled::ResumeOriginal => {
                // A verifies whether this is first execution, a partly completed
                // transition or replay. B never infers success from changed counts.
                port.collect_group(hold, &pending.intent).await?;
                Ok(Step::OriginalSubmitted)
            }
        }
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
                    let intent = port
                        .retain_evidence(hold.clone(), checkpoint, *group)
                        .await?;
                    if !intent.valid()
                        || intent.group != *group
                        || intent.binding != checkpoint.binding
                        || intent.before_revision != checkpoint.source_revision
                    {
                        return Err(Refusal::InvalidEvidence);
                    }
                    port.verify_retained(&intent).await?;
                    port.select(checkpoint, intent).await?;
                    Ok(Step::EvidenceSelected)
                }
            },
        }
    }
}

#[cfg(test)]
#[path = "ledger_collector/tests.rs"]
mod tests;
