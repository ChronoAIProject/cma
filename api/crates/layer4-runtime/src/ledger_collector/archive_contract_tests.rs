//! Addendum-2 contract doubles; real archive/source qualification is separate.
use super::*;

fn staged(model: &Model) -> Intent {
    assert_eq!(run(step(enabled(), model)), Ok(Step::PagePinned));
    assert_eq!(run(step(enabled(), model)), Ok(Step::ReferenceRecorded));
    run(model.load()).unwrap().staged.unwrap().intent
}

#[test]
fn retain_reply_loss_recovers_exact_reference_before_selected_with_admission_closed() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(format, &[(1, Decision::SettledDeleted)]);
        assert_eq!(run(step(enabled(), &model)), Ok(Step::PagePinned));
        model.state.lock().unwrap().fail_after = Some(Call::Retain);
        assert_eq!(run(step(enabled(), &model)), Err(Refusal::UnknownOutcome));
        let original = {
            let mut s = model.state.lock().unwrap();
            assert_eq!(s.retentions, 1);
            assert!(s.checkpoint.staged.is_none());
            assert!(s.checkpoint.pending.is_none());
            assert!(s.selected.is_empty());
            s.checkpoint.fence += 1;
            s.checkpoint.revision += 1;
            s.evidence.values().next().unwrap().clone()
        };
        let reopened = model.reopened();
        assert_eq!(
            run(step(Admission::default(), &reopened)),
            Ok(Step::ReferenceRecovered)
        );
        let saved = run(reopened.load()).unwrap().staged.unwrap();
        assert_eq!(saved.intent, original);
        assert_eq!(saved.reference, Some(original.reference()));
        assert_eq!(
            run(step(Admission::default(), &reopened)),
            Ok(Step::EvidenceSelected)
        );
        run(until_complete(&reopened, 16)).unwrap();
        let s = model.state.lock().unwrap();
        assert_eq!(s.retentions, 1);
        assert_eq!(s.selected, [original.operation]);
        assert_eq!(s.mutations, 1);
    }
}

#[test]
fn source_commit_without_cold_completion_cannot_advance_cursor() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(format, &[(1, Decision::SettledDeleted)]);
        selected(&model);
        let before = run(model.load()).unwrap();
        let intent = before.pending.as_ref().unwrap().intent.clone();
        model.state.lock().unwrap().fail_after = Some(Call::Source);
        assert_eq!(run(step(enabled(), &model)), Err(Refusal::UnknownOutcome));
        assert_eq!(run(model.load()).unwrap(), before);
        assert_eq!(
            run(model.collection_completion(&intent, &intent.reference())),
            Ok(None)
        );
        {
            let mut s = model.state.lock().unwrap();
            assert_eq!(s.mutations, 1);
            assert_eq!(s.archive_pending, Some(intent.operation));
            assert!(s.source_committed.is_some());
            assert!(s.cold.is_empty());
            s.unavailable = Some(Refusal::ArchiveUnavailable);
        }
        assert_eq!(
            run(step(enabled(), &model)),
            Err(Refusal::ArchiveUnavailable)
        );
        assert_eq!(run(model.load()).unwrap(), before);
        model.state.lock().unwrap().unavailable = None;
        assert_eq!(
            run(step(Admission::default(), &model.reopened())),
            Ok(Step::OriginalSubmitted)
        );
        assert_eq!(
            run(model.load()).unwrap(),
            before,
            "a reply is not persisted cold evidence"
        );
        assert_eq!(
            run(step(Admission::default(), &model)),
            Ok(Step::ReceiptRecorded)
        );
        let recorded = run(model.load()).unwrap();
        assert!(recorded.after_group.is_none());
        assert!(recorded.pending.unwrap().completion.is_some());
        assert_eq!(
            run(step(Admission::default(), &model)),
            Ok(Step::GroupSettled)
        );
        let s = model.state.lock().unwrap();
        assert_eq!(s.mutations, 1);
        assert!(s.archive_pending.is_none() && s.source_committed.is_none());
        assert_eq!(s.checkpoint.after_group, Some(intent.group));
    }
}

#[test]
fn every_mutating_group_uses_fresh_revision_observation_and_operation() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(
            format,
            &[(1, Decision::SettledDeleted), (2, Decision::SettledDeleted)],
        );
        selected(&model);
        let first = run(model.load()).unwrap().pending.unwrap().intent;
        for expected in [
            Step::OriginalSubmitted,
            Step::ReceiptRecorded,
            Step::GroupSettled,
        ] {
            assert_eq!(run(step(enabled(), &model)), Ok(expected));
        }
        let original = run(model.collection_completion(&first, &first.reference()))
            .unwrap()
            .unwrap();
        let after = run(model.load()).unwrap();
        assert!(after.page.is_none() && after.cursor.is_none());
        selected(&model);
        let second = run(model.load()).unwrap().pending.unwrap().intent;
        assert_ne!(first.operation, second.operation);
        assert_ne!(first.group, second.group);
        assert_ne!(first.observation_sha256, second.observation_sha256);
        assert_ne!(first.evidence_sha256, second.evidence_sha256);
        assert_eq!(first.plan_sha256, second.plan_sha256);
        assert_eq!(second.before_revision, original.after_revision);
        run(until_complete(&model, 16)).unwrap();
        assert_eq!(
            run(model
                .reopened()
                .collection_completion(&first, &first.reference())),
            Ok(Some(original))
        );
        let s = model.state.lock().unwrap();
        assert_eq!(s.mutations, 2);
        assert_eq!(s.retentions, 2);
        assert_eq!(s.cold.len(), 2);
    }
}

#[test]
fn retained_classifications_do_not_call_the_mutation_archive() {
    let model = Model::new(
        SourceFormat::V2,
        &[(1, Decision::RetainLive), (2, Decision::RetainUnknown)],
    );
    run(until_complete(&model, 16)).unwrap();
    let s = model.state.lock().unwrap();
    assert_eq!(s.groups.len(), 2);
    assert_eq!(s.mutations, 0);
    assert_eq!(s.retentions, 0);
    assert!(s.evidence.is_empty() && s.cold.is_empty() && s.retained_originals.is_empty());
    assert!(s.archive_pending.is_none() && s.staged.is_none());
}

#[test]
fn absent_or_changed_exact_retained_bytes_refuse_before_selected() {
    for case in 0..4 {
        let model = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
        let original = staged(&model);
        let before = run(model.load()).unwrap();
        {
            let mut s = model.state.lock().unwrap();
            match case {
                0 => {
                    s.retained_originals.remove(&original.operation);
                }
                1 => {
                    s.retained_originals.get_mut(&original.operation).unwrap()[2].push(b' ');
                }
                2 => {
                    s.retained_originals.get_mut(&original.operation).unwrap()[1] =
                        vec![b' '; 64 * 1024 + 1];
                }
                _ => {
                    s.evidence.get_mut(&original.operation).unwrap().plan_sha256 = digest(7);
                }
            }
        }
        assert_eq!(
            run(step(enabled(), &model)),
            Err(Refusal::InvalidEvidence),
            "case {case}"
        );
        assert_eq!(run(model.load()).unwrap(), before);
        let s = model.state.lock().unwrap();
        assert!(s.selected.is_empty());
        assert_eq!(s.mutations, 0);
        assert_eq!(s.next_operation, 2);
    }
}

#[test]
fn changed_or_lost_cold_receipt_after_recording_keeps_cursor_pinned() {
    for missing in [false, true] {
        let model = Model::new(SourceFormat::V1, &[(1, Decision::SettledDeleted)]);
        selected(&model);
        run(step(enabled(), &model)).unwrap();
        assert_eq!(run(step(enabled(), &model)), Ok(Step::ReceiptRecorded));
        let before = run(model.load()).unwrap();
        let operation = before.pending.as_ref().unwrap().intent.operation;
        {
            let mut s = model.state.lock().unwrap();
            if missing {
                s.cold.remove(&operation);
            } else {
                s.cold.get_mut(&operation).unwrap().after_revision += 1;
            }
        }
        let error = if missing {
            Refusal::UnknownOutcome
        } else {
            Refusal::InvalidEvidence
        };
        assert_eq!(run(step(enabled(), &model)), Err(error));
        assert_eq!(run(model.load()).unwrap(), before);
        assert_eq!(model.state.lock().unwrap().mutations, 1);
    }
}

#[test]
fn nonfinal_or_mismatched_receipt_fields_never_advance_progress() {
    for case in 0..13 {
        let model = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
        selected(&model);
        run(step(enabled(), &model)).unwrap();
        let before = run(model.load()).unwrap();
        let operation = before.pending.as_ref().unwrap().intent.operation;
        {
            let mut s = model.state.lock().unwrap();
            let r = s.cold.get_mut(&operation).unwrap();
            match case {
                0 => r.version += 1,
                1 => r.phase = Phase::Prepared,
                2 => r.phase = Phase::SourceCommitted,
                3 => r.operation = [9; 16],
                4 => r.binding_sha256 = digest(9),
                5 => r.plan_sha256 = digest(9),
                6 => r.group = digest(9),
                7 => r.source_sha256 = digest(9),
                8 => r.observation_sha256 = digest(9),
                9 => r.before_revision += 1,
                10 => r.after_revision = r.before_revision,
                11 => r.before_sha256 = [0; 32],
                _ => r.sha256 = [0; 32],
            }
        }
        assert_eq!(
            run(step(enabled(), &model)),
            Err(Refusal::InvalidEvidence),
            "case {case}"
        );
        assert_eq!(run(model.load()).unwrap(), before);
        assert_eq!(model.state.lock().unwrap().mutations, 1);
    }
}

#[test]
fn staged_original_survives_revision_change_before_selected() {
    let model = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
    let original = staged(&model);
    model.state.lock().unwrap().source_revision += 1;
    assert_eq!(
        run(step(Admission::default(), &model)),
        Ok(Step::EvidenceSelected)
    );
    assert_eq!(
        run(step(Admission::default(), &model)),
        Err(Refusal::UnknownOutcome)
    );
    assert_eq!(run(model.load()).unwrap().pending.unwrap().intent, original);
    let s = model.state.lock().unwrap();
    assert_eq!(s.mutations, 0);
    assert_eq!(s.next_operation, 2);
}
