use layer2_core::account_deletion::{
    AccountDeletion, AccountDeletionError, AccountDeletionStatus, ErasureEvent, ErasureEventKind,
    ErasureStep,
};

/// The general writer cannot consume an independently stored verification proof.
/// A future verified-completion transaction must own terminal writes separately.
pub(super) fn require_pending(
    deletion: &AccountDeletion,
    events: &[ErasureEvent],
) -> Result<(), AccountDeletionError> {
    if deletion.status == AccountDeletionStatus::Completed
        || deletion.completed_at.is_some()
        || deletion.expire_at.is_some()
        || deletion.reverify_at.is_some()
        || deletion
            .steps
            .iter()
            .any(|progress| progress.step == ErasureStep::VerifyNoResidue)
        || events.iter().any(|event| {
            event.step == ErasureStep::VerifyNoResidue
                && matches!(event.kind, ErasureEventKind::Completed { .. })
        })
    {
        return Err(AccountDeletionError::VerificationRequired);
    }
    Ok(())
}

pub(super) fn preserve_acceptance(
    old: &AccountDeletion,
    new: &AccountDeletion,
) -> Result<(), AccountDeletionError> {
    if old.status == AccountDeletionStatus::Completed {
        return Err(AccountDeletionError::VerificationRequired);
    }
    if old.id != new.id
        || old.subject != new.subject
        || old.subject_hash != new.subject_hash
        || old.active_subject_hash != new.active_subject_hash
        || old.requested_at != new.requested_at
        || old.deadline_at != new.deadline_at
    {
        return Err(AccountDeletionError::Invalid);
    }
    Ok(())
}
