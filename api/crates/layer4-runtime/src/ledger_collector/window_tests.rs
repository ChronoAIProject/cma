//! Async ownership tests against the model; no claim of physical exclusion.
use super::*;
use crate::ledger_collector::owned_window::{start, WindowBudget, WindowOutcome};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

fn held_model(format: SourceFormat) -> Arc<Model> {
    let model = Arc::new(Model::new(
        format,
        &[(1, Decision::SettledDeleted), (2, Decision::SettledDeleted)],
    ));
    model.state.lock().unwrap().required_hold = Some(model.holds.clone());
    model
}

async fn dispatch_waiting(model: &Model) {
    for _ in 0..32 {
        if model.state.lock().unwrap().in_flight.is_some() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("dispatch never reached the controlled suspension");
}

fn finish_dispatch(model: &Model) {
    let mut state = model.state.lock().unwrap();
    state.in_flight.take();
    state.suspend_dispatch = false;
    model.resume.notify_one();
}

#[tokio::test]
async fn one_gate_spans_selection_source_and_all_progress_publications() {
    for format in [SourceFormat::V1, SourceFormat::V2] {
        let model = held_model(format);
        assert_eq!(
            start(enabled(), model.clone(), WindowBudget::default())
                .receive()
                .await,
            Ok(WindowOutcome::Stopped(Step::GroupSettled))
        );
        assert_eq!(model.acquires.load(Ordering::SeqCst), 1);
        assert_eq!(model.holds.load(Ordering::SeqCst), 0);
        let state = model.state.lock().unwrap();
        assert_eq!(state.mutations, 1);
        assert!(state.checkpoint.pending.is_none());
        assert_eq!(
            state.groups.len(),
            1,
            "one window settles at most one group"
        );
    }
}

#[tokio::test]
async fn dropping_reply_does_not_cancel_admitted_window() {
    let model = held_model(SourceFormat::V2);
    model.state.lock().unwrap().suspend_dispatch = true;
    let reply = start(enabled(), model.clone(), WindowBudget::default());
    dispatch_waiting(&model).await;
    drop(reply);
    assert_eq!(model.holds.load(Ordering::SeqCst), 1);
    finish_dispatch(&model);
    for _ in 0..32 {
        tokio::task::yield_now().await;
        if model.state.lock().unwrap().checkpoint.pending.is_none() {
            break;
        }
    }
    assert_eq!(model.holds.load(Ordering::SeqCst), 0);
    let state = model.state.lock().unwrap();
    assert_eq!(state.mutations, 1);
    assert!(state.checkpoint.pending.is_none());
    assert_eq!(state.cold.len(), 1);
}

#[tokio::test(start_paused = true)]
async fn deadline_preserves_original_pending_and_backend_owned_work() {
    let model = held_model(SourceFormat::V1);
    model.state.lock().unwrap().suspend_dispatch = true;
    let reply = start(enabled(), model.clone(), WindowBudget::default());
    dispatch_waiting(&model).await;
    let original = model.state.lock().unwrap().checkpoint.pending.clone();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert_eq!(reply.receive().await, Err(Refusal::UnknownOutcome));
    assert_eq!(model.holds.load(Ordering::SeqCst), 1);
    assert_eq!(model.state.lock().unwrap().checkpoint.pending, original);
    assert_eq!(step(enabled(), &*model).await, Err(Refusal::Conflict));
    finish_dispatch(&model);
    assert_eq!(
        start(Admission::default(), model.clone(), WindowBudget::default())
            .receive()
            .await,
        Ok(WindowOutcome::Stopped(Step::GroupSettled))
    );
    let state = model.state.lock().unwrap();
    assert_eq!(state.mutations, 1);
    assert_eq!(state.selected.len(), 1);
}

#[tokio::test]
async fn takeover_does_not_let_old_window_adopt_new_worker_fence() {
    let model = held_model(SourceFormat::V2);
    model.state.lock().unwrap().suspend_dispatch = true;
    let reply = start(enabled(), model.clone(), WindowBudget::default());
    dispatch_waiting(&model).await;
    {
        let mut state = model.state.lock().unwrap();
        state.checkpoint.fence += 1;
        state.checkpoint.revision += 1;
    }
    finish_dispatch(&model);
    assert_eq!(reply.receive().await, Err(Refusal::Conflict));
    {
        let state = model.state.lock().unwrap();
        assert!(state
            .checkpoint
            .pending
            .as_ref()
            .unwrap()
            .receipts
            .is_empty());
        assert_eq!(state.mutations, 1);
    }
    assert_eq!(
        start(Admission::default(), model.clone(), WindowBudget::default())
            .receive()
            .await,
        Ok(WindowOutcome::Stopped(Step::GroupSettled))
    );
    assert_eq!(model.state.lock().unwrap().mutations, 1);
}

#[tokio::test]
async fn step_budget_yields_without_discarding_selected_work() {
    let model = held_model(SourceFormat::V2);
    let budget = WindowBudget {
        max_steps: 2,
        ..WindowBudget::default()
    };
    assert_eq!(
        start(enabled(), model.clone(), budget).receive().await,
        Ok(WindowOutcome::BudgetExhausted)
    );
    {
        let state = model.state.lock().unwrap();
        assert!(state.checkpoint.pending.is_some());
        assert_eq!(state.mutations, 0);
    }
    assert_eq!(
        start(Admission::default(), model.clone(), WindowBudget::default())
            .receive()
            .await,
        Ok(WindowOutcome::Stopped(Step::GroupSettled))
    );
    let state = model.state.lock().unwrap();
    assert_eq!(state.selected.len(), 1);
    assert_eq!(state.mutations, 1);
    assert!(state.groups.contains_key(&digest(2)));
}

#[tokio::test]
async fn closed_admission_and_invalid_budgets_never_acquire_authority() {
    let model = held_model(SourceFormat::V1);
    for budget in [
        WindowBudget {
            max_steps: 0,
            ..WindowBudget::default()
        },
        WindowBudget {
            max_steps: 65,
            ..WindowBudget::default()
        },
        WindowBudget {
            deadline: Duration::ZERO,
            ..WindowBudget::default()
        },
        WindowBudget {
            deadline: Duration::from_secs(31),
            ..WindowBudget::default()
        },
    ] {
        assert_eq!(
            start(enabled(), model.clone(), budget).receive().await,
            Err(Refusal::InvalidEvidence)
        );
    }
    assert_eq!(model.loads.load(Ordering::SeqCst), 0);
    assert_eq!(
        start(Admission::default(), model.clone(), WindowBudget::default())
            .receive()
            .await,
        Ok(WindowOutcome::Stopped(Step::Disabled))
    );
    assert_eq!(model.acquires.load(Ordering::SeqCst), 0);
    assert_eq!(model.state.lock().unwrap().mutations, 0);
}
