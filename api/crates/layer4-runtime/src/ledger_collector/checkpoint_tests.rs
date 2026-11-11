//! Reject inconsistent restart projections before any authority or progress use.
use super::*;
use std::sync::atomic::Ordering;

#[test]
fn restart_refuses_pending_group_that_would_skip_earlier_work() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(
            format,
            &[(1, Decision::SettledDeleted), (2, Decision::RetainLive)],
        );
        selected(&model);
        {
            let mut state = model.state.lock().unwrap();
            let pending = state.checkpoint.pending.as_mut().unwrap();
            pending.intent.group = digest(2);
            pending.intent.decision = Decision::RetainLive;
            pending.reference = None;
            let intent = pending.intent.clone();
            state.evidence.insert(intent.operation, intent);
        }
        assert_restart_refuses_unchanged(&model);
    }
}

#[test]
fn restart_refuses_pages_behind_or_at_the_settled_fair_position() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        for after_group in [digest(1), digest(2)] {
            let model = Model::new(
                format,
                &[(1, Decision::RetainLive), (3, Decision::SettledDeleted)],
            );
            run(step(enabled(), &model)).unwrap();
            model.state.lock().unwrap().checkpoint.after_group = Some(after_group);
            assert_restart_refuses_unchanged(&model);
        }
    }
}

#[test]
fn restart_refuses_completed_inventory_with_unconsumed_cursor() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = Model::new(format, &[]);
        run(until_complete(&model, 4)).unwrap();
        model.state.lock().unwrap().checkpoint.cursor = Some("unfinished-page".into());
        assert_restart_refuses_unchanged(&model);
    }
}

fn assert_restart_refuses_unchanged(model: &Model) {
    let before = run(model.load()).unwrap();
    let acquired = model.acquires.load(Ordering::SeqCst);
    let selected = model.state.lock().unwrap().selected.clone();
    assert_eq!(
        run(step(enabled(), &model.reopened())),
        Err(Refusal::InvalidEvidence)
    );
    assert_eq!(run(model.load()).unwrap(), before);
    assert_eq!(model.acquires.load(Ordering::SeqCst), acquired);
    let state = model.state.lock().unwrap();
    assert_eq!(state.selected, selected);
    assert_eq!(state.mutations, 0);
    assert!(state.cold.is_empty());
}
