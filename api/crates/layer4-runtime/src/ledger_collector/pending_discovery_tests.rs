use super::*;
use std::sync::atomic::Ordering;

fn retained_without_b_reference(model: &Model) -> Intent {
    assert_eq!(run(step(enabled(), model)), Ok(Step::PagePinned));
    model.state.lock().unwrap().fail_after = Some(Call::Retain);
    assert_eq!(run(step(enabled(), model)), Err(Refusal::UnknownOutcome));
    let s = model.state.lock().unwrap();
    assert!(!s.checkpoint.has_work());
    s.evidence.values().next().unwrap().clone()
}

#[test]
fn pending_archive_is_recovered_before_refresh_or_enumeration() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(format, &[(1, Decision::SettledDeleted)]);
        let original = retained_without_b_reference(&model);
        let enumerated = {
            let mut s = model.state.lock().unwrap();
            s.checkpoint.page = None;
            s.source_revision += 1;
            s.enumerations
        };
        assert_eq!(
            run(step(Admission::default(), &model.reopened())),
            Ok(Step::ReferenceRecovered)
        );
        let checkpoint = run(model.load()).unwrap();
        assert_eq!(checkpoint.source_revision, original.before_revision);
        assert_eq!(checkpoint.staged.unwrap().intent, original);
        assert_eq!(model.state.lock().unwrap().enumerations, enumerated);
        assert_eq!(
            run(step(Admission::default(), &model)),
            Ok(Step::EvidenceSelected)
        );
        assert_eq!(
            run(step(Admission::default(), &model)),
            Err(Refusal::UnknownOutcome)
        );
        assert_eq!(model.state.lock().unwrap().mutations, 0);
    }
}

#[test]
fn malformed_or_missing_pending_originals_never_fall_back_to_new_work() {
    for case in 0..9 {
        let model = Model::new(SourceFormat::V2, &[(1, Decision::SettledDeleted)]);
        let original = retained_without_b_reference(&model);
        let before = run(model.load()).unwrap();
        let acquired = model.acquires.load(Ordering::SeqCst);
        {
            let mut s = model.state.lock().unwrap();
            match case {
                0 => {
                    s.evidence.remove(&original.operation);
                }
                1 => {
                    s.retained_originals.remove(&original.operation);
                }
                2 => {
                    s.references.remove(&original.operation);
                }
                _ => {
                    let r = s.references.get_mut(&original.operation).unwrap();
                    match case {
                        3 => r.archive_version = 2,
                        4 => r.archive_id = uuid::Uuid::nil().to_string(),
                        5 => r.artifact_lengths[0] = 0,
                        6 => r.artifact_lengths[1] = 65_537,
                        7 => r.artifact_lengths[2] += 1,
                        _ => r.format = "future-reference".into(),
                    }
                }
            }
        }
        assert_eq!(
            run(step(enabled(), &model)),
            Err(Refusal::InvalidEvidence),
            "case {case}"
        );
        assert_eq!(run(model.load()).unwrap(), before);
        assert_eq!(model.acquires.load(Ordering::SeqCst), acquired);
        let s = model.state.lock().unwrap();
        assert_eq!(s.next_operation, 2);
        assert_eq!(s.enumerations, 1);
        assert_eq!(s.mutations, 0);
        assert!(s.selected.is_empty());
    }
}

#[test]
fn inconsistent_fair_position_does_not_discard_an_archive_pending_operation() {
    let model = Model::new(SourceFormat::V1, &[(1, Decision::SettledDeleted)]);
    let original = retained_without_b_reference(&model);
    {
        let mut s = model.state.lock().unwrap();
        s.checkpoint.page = None;
        s.checkpoint.after_group = Some(original.group);
    }
    let before = run(model.load()).unwrap();
    assert_eq!(run(step(enabled(), &model)), Err(Refusal::Conflict));
    assert_eq!(run(model.load()).unwrap(), before);
    assert_eq!(
        model.state.lock().unwrap().archive_pending,
        Some(original.operation)
    );
}
