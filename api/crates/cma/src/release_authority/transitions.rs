use super::{validation::increment, *};

pub(super) fn begin(head: &ReleaseHead, request: &TransitionRequest) -> Result<ReleaseHead> {
    head.validate()?;
    OperationRequest::Transition(Box::new(request.clone())).validate()?;
    if !matches!(head.phase, Phase::Active | Phase::FailedHold)
        || head.epoch != request.expected_epoch
        || head.control_revision != request.expected_control_revision
        || head.deployment != request.deployment
        || head.pair != request.before
        || !head.protected_pairs.contains(&request.after)
        || request.next_writer.fence != increment(head.writer_fence)?
        || head
            .writer
            .as_ref()
            .is_some_and(|w| w.instance_id == request.next_writer.instance_id)
        || (head.phase == Phase::FailedHold && head.active_effects != 0)
    {
        return Err(AuthorityError::Conflict);
    }
    let mut next = head.clone();
    next.phase = Phase::Freezing;
    next.control_revision = increment(head.control_revision)?;
    next.operation_id = Some(request.operation_id.clone());
    Ok(next)
}

pub(super) fn advance(
    head: &ReleaseHead,
    operation: &ReleaseOperation,
    request: &AdvanceRequest,
) -> Result<ReleaseHead> {
    head.validate()?;
    operation.validate()?;
    request.operator.validate()?;
    let OperationRequest::Transition(original) = &operation.request else {
        return Err(AuthorityError::Conflict);
    };
    if request.deployment != head.deployment
        || request.operation_id != original.operation_id
        || head.operation_id.as_deref() != Some(&request.operation_id)
        || request.expected_epoch != head.epoch
        || request.expected_control_revision != head.control_revision
        || request.expected_phase != head.phase
        || operation.terminal_recovery_receipt.is_some()
    {
        return Err(AuthorityError::Conflict);
    }
    // Reasserting an existing hold records no new transition. In particular,
    // an operator cannot exhaust recovery capacity by repeatedly holding it.
    if request.action == AdvanceAction::Hold
        && matches!(head.phase, Phase::AbortedHold | Phase::FailedHold)
    {
        return Ok(head.clone());
    }
    // One reserved terminal receipt remains available even at the ordinary
    // audit bound. It can only close this operation, never enable another step.
    let terminal = matches!(
        request.action,
        AdvanceAction::Abort | AdvanceAction::Activate
    );
    if operation.receipts.len() >= 32 && !terminal {
        return Err(AuthorityError::Conflict);
    }
    let mut next = head.clone();
    next.control_revision = increment(head.control_revision)?;
    next.phase = match (head.phase, request.action) {
        (Phase::Freezing, AdvanceAction::ConfirmDrained) => Phase::Frozen,
        (Phase::Frozen, AdvanceAction::Prepare) => Phase::Prepared,
        (Phase::Prepared, AdvanceAction::Commit) => {
            if head.pair != original.before
                || head.epoch != original.expected_epoch
                || original.next_writer.fence != increment(head.writer_fence)?
            {
                return Err(AuthorityError::Conflict);
            }
            next.pair = original.after.clone();
            next.epoch = increment(head.epoch)?;
            next.writer_fence = increment(head.writer_fence)?;
            next.writer = None;
            Phase::CommittedVerifying
        }
        (Phase::CommittedVerifying | Phase::FailedHold, AdvanceAction::Activate) => {
            if head.pair != original.after || head.epoch != original.next_writer.epoch {
                return Err(AuthorityError::Conflict);
            }
            next.writer = Some(original.next_writer.clone());
            next.operation_id = None;
            Phase::Active
        }
        (
            Phase::Freezing | Phase::Frozen | Phase::Prepared | Phase::AbortedHold,
            AdvanceAction::Hold,
        ) => Phase::AbortedHold,
        (Phase::CommittedVerifying | Phase::FailedHold, AdvanceAction::Hold) => Phase::FailedHold,
        (
            Phase::Freezing | Phase::Frozen | Phase::Prepared | Phase::AbortedHold,
            AdvanceAction::Abort,
        ) => {
            if head.pair != original.before || head.epoch != original.expected_epoch {
                return Err(AuthorityError::Conflict);
            }
            match (&head.writer, &operation.predecessor_operation_id) {
                (Some(_), None) => {
                    next.operation_id = None;
                    Phase::Active
                }
                (None, Some(predecessor)) => {
                    next.operation_id = Some(predecessor.clone());
                    Phase::FailedHold
                }
                _ => return Err(AuthorityError::Conflict),
            }
        }
        _ => return Err(AuthorityError::Conflict),
    };
    if request.action != AdvanceAction::Hold && head.active_effects != 0 {
        return Err(AuthorityError::EvidenceRequired);
    }
    next.validate()?;
    Ok(next)
}

pub(super) fn action(value: AdvanceAction) -> EvidenceAction {
    match value {
        AdvanceAction::ConfirmDrained => EvidenceAction::ConfirmDrained,
        AdvanceAction::Prepare => EvidenceAction::Prepare,
        AdvanceAction::Commit => EvidenceAction::Commit,
        AdvanceAction::Activate => EvidenceAction::Activate,
        AdvanceAction::Hold => EvidenceAction::Hold,
        AdvanceAction::Abort => EvidenceAction::Abort,
    }
}
