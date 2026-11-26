use super::super::reconciliation::{
    EffectProgress, EffectReconciler, EffectStatusProvider, ProviderEffectObservation,
};
use super::*;

mod admission;
mod budget;
pub(super) mod fixture;
use fixture::{append, setup};
use wire::EffectOutcome as Outcome;

#[tokio::test]
async fn retained_terminal_outcome_is_not_queried_again_after_manager_replacement() {
    for outcome in [Outcome::CommitConfirmed, Outcome::Rejected] {
        let (store, provider, worker, registration) = setup(1);
        provider
            .reply
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .outcome = outcome;
        let result = worker.reconcile(&registration).await.unwrap();
        assert_eq!(
            result,
            EffectProgress {
                terminal: 1,
                unresolved: 0,
                failures: vec![]
            }
        );
        let replacement = EffectReconciler::new(store.clone(), provider.clone());
        assert_eq!(replacement.reconcile(&registration).await.unwrap(), result);
        assert_eq!(provider.calls.lock().unwrap().len(), 1);
        let state = store.0.lock().unwrap();
        let effect = &state.effects.as_ref().unwrap()[0];
        assert_eq!(effect.observations.len(), 1);
        assert_eq!(effect.observations[0].outcome, outcome);
        assert_eq!(
            &provider.calls.lock().unwrap()[0],
            &effect.permission.intent
        );
        assert_eq!(
            effect.replay_prepared(&effect.permission.intent),
            Err(ExecutionError::Unknown)
        );
    }
}

#[tokio::test]
async fn status_failure_or_missing_lookup_stays_unknown_without_new_permissions() {
    for reply in [
        Ok(None),
        Err(ExecutionError::Unavailable),
        Err(ExecutionError::Unknown),
    ] {
        let (store, provider, worker, registration) = setup(1);
        // Absence of provider idempotency is never permission to resend.
        let mut row = member(1);
        row.effect_count = 1;
        let mut effect = fixture::effect(&row, 1);
        effect.permission.intent.provider_idempotency = None;
        use sha2::{Digest, Sha256};
        effect.permission.permission_id = wire::Digest::new(&format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&("resource-effect-permission-v1", &effect.permission.intent))
                    .unwrap()
            )
        ))
        .unwrap();
        store.0.lock().unwrap().effects = Some(vec![effect]);
        *provider.reply.lock().unwrap() = reply;
        for _ in 0..3 {
            let result = worker.reconcile(&registration).await.unwrap();
            assert_eq!(
                (result.terminal, result.unresolved, result.failures.len()),
                (0, 1, 1)
            );
        }
        assert!(store.0.lock().unwrap().effects.as_ref().unwrap()[0]
            .observations
            .is_empty());
        assert!(provider
            .calls
            .lock()
            .unwrap()
            .iter()
            .all(|intent| intent.provider_idempotency.is_none()));
    }
}

#[tokio::test]
async fn exact_intent_and_admissible_outcomes_are_required_before_append() {
    for field in 0..10 {
        let (store, provider, worker, registration) = setup(1);
        {
            let mut reply = provider.reply.lock().unwrap();
            let status = reply.as_mut().unwrap().as_mut().unwrap();
            match field {
                0 => status.intent.effect_id = id("other-effect"),
                1 => status.intent.execution.receiver.boot = id("other-boot"),
                2 => status.intent.registration = digest('f'),
                3 => status.intent.operation = wire::EffectOperationClass::ModelRequest,
                4 => status.intent.request_digest = digest('f'),
                5 => status.intent.provider_idempotency = Some(digest('f')),
                6 => status.intent.provider_idempotency = None,
                7 => {
                    let wire::ExecutionBasisV1::Share { grant } = &mut status.intent.grant else {
                        panic!()
                    };
                    grant.family.generation = generation(2);
                }
                8 => status.outcome = Outcome::Prepared,
                _ => status.outcome = Outcome::SubmissionUnknown,
            }
        }
        let result = worker.reconcile(&registration).await.unwrap();
        assert_eq!(
            result.failures,
            vec![(id("effect-1"), ExecutionError::EvidenceMismatch)],
            "field {field}"
        );
        assert_eq!((result.terminal, result.unresolved), (0, 1));
        assert!(store.0.lock().unwrap().effects.as_ref().unwrap()[0]
            .observations
            .is_empty());
    }
}

#[tokio::test]
async fn unchanged_pending_status_does_not_fill_the_evidence_ledger() {
    for outcome in [Outcome::Submitted, Outcome::ReconciliationRequired] {
        let (store, provider, worker, registration) = setup(1);
        provider
            .reply
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .as_mut()
            .unwrap()
            .outcome = outcome;
        for _ in 0..MAX_EFFECT_OBSERVATIONS * 2 {
            assert_eq!(
                worker.reconcile(&registration).await.unwrap(),
                EffectProgress {
                    terminal: 0,
                    unresolved: 1,
                    failures: vec![]
                }
            );
        }
        assert_eq!(
            store.0.lock().unwrap().effects.as_ref().unwrap()[0]
                .observations
                .len(),
            1
        );
    }
}

#[tokio::test]
async fn final_outcome_keeps_its_reserved_slot_at_evidence_capacity() {
    let (store, provider, worker, registration) = setup(1);
    {
        let mut state = store.0.lock().unwrap();
        let effect = &mut state.effects.as_mut().unwrap()[0];
        append(effect, Outcome::SubmissionUnknown);
        for _ in 1..MAX_EFFECT_OBSERVATIONS - 1 {
            append(effect, Outcome::ReconciliationRequired);
        }
    }
    {
        let mut reply = provider.reply.lock().unwrap();
        let status = reply.as_mut().unwrap().as_mut().unwrap();
        status.outcome = Outcome::ReconciliationRequired;
        status.evidence_digest = digest('e');
    }
    let result = worker.reconcile(&registration).await.unwrap();
    assert_eq!(
        result.failures,
        vec![(id("effect-1"), ExecutionError::Capacity)]
    );
    provider
        .reply
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .outcome = Outcome::CommitConfirmed;
    assert_eq!(worker.reconcile(&registration).await.unwrap().terminal, 1);
    assert_eq!(
        store.0.lock().unwrap().effects.as_ref().unwrap()[0]
            .observations
            .len(),
        MAX_EFFECT_OBSERVATIONS
    );
}

#[tokio::test]
async fn ambiguous_append_is_recovered_from_the_original_ledger() {
    let (store, provider, worker, registration) = setup(1);
    store.0.lock().unwrap().fault = Some(WriteFault::Effect);
    let uncertain = worker.reconcile(&registration).await.unwrap();
    assert_eq!(
        uncertain.failures,
        vec![(id("effect-1"), ExecutionError::Unknown)]
    );
    assert_eq!(uncertain.terminal, 0);
    let replacement = EffectReconciler::new(store.clone(), provider.clone());
    assert_eq!(
        replacement.reconcile(&registration).await.unwrap().terminal,
        1
    );
    assert_eq!(provider.calls.lock().unwrap().len(), 1);
    assert_eq!(
        store.0.lock().unwrap().effects.as_ref().unwrap()[0]
            .observations
            .len(),
        1
    );
}

#[tokio::test]
async fn competing_managers_append_one_identical_terminal_observation() {
    let (store, provider, first, registration) = setup(1);
    *provider.gate.lock().unwrap() = Some(Arc::new(tokio::sync::Barrier::new(2)));
    let second = EffectReconciler::new(store.clone(), provider);
    let (a, b) = tokio::join!(
        first.reconcile(&registration),
        second.reconcile(&registration)
    );
    assert_eq!(a.unwrap().terminal, 1);
    assert_eq!(b.unwrap().terminal, 1);
    assert_eq!(
        store.0.lock().unwrap().effects.as_ref().unwrap()[0]
            .observations
            .len(),
        1
    );
}

#[tokio::test]
async fn full_ledger_is_paginated_and_all_identities_validate_before_lookup() {
    let (store, provider, worker, registration) = setup(MAX_MEMBER_EFFECTS);
    *provider.reply.lock().unwrap() = Ok(None);
    let result = worker.reconcile(&registration).await.unwrap();
    assert_eq!(result.unresolved, MAX_MEMBER_EFFECTS);
    assert_eq!(store.0.lock().unwrap().effect_pages, [0, 32, 64, 96]);
    assert_eq!(
        provider.calls.lock().unwrap().len(),
        MAX_MEMBER_EFFECTS as usize
    );
    for defect in 0..5 {
        let (store, provider, worker, registration) = setup(33);
        {
            let mut state = store.0.lock().unwrap();
            let effects = state.effects.as_mut().unwrap();
            match defect {
                0 => {
                    effects.pop();
                }
                1 => {
                    effects[32] = effects[0].clone();
                }
                2 => {
                    effects[32].permission.permission_id = digest('f');
                }
                3 => {
                    effects[32] = fixture::effect(&member(2), 33);
                }
                _ => {
                    effects[32] = effects[0].clone();
                    effects[32].permission.ledger_sequence = generation(33);
                }
            }
        }
        assert!(
            worker.reconcile(&registration).await.is_err(),
            "defect {defect}"
        );
        assert!(provider.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test(start_paused = true)]
async fn stalled_lookup_is_bounded_and_does_not_block_another_effect() {
    let (store, provider, worker, registration) = setup(2);
    let second = store.0.lock().unwrap().effects.as_ref().unwrap()[1].clone();
    provider
        .reply
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .as_mut()
        .unwrap()
        .intent = second.permission.intent;
    *provider.stalled.lock().unwrap() = Some(id("effect-1"));
    let result = worker.reconcile(&registration).await.unwrap();
    assert_eq!((result.terminal, result.unresolved), (1, 1));
    assert_eq!(
        result.failures,
        vec![(id("effect-1"), ExecutionError::Unknown)]
    );
    let state = store.0.lock().unwrap();
    assert!(state.effects.as_ref().unwrap()[0].observations.is_empty());
    assert_eq!(state.effects.as_ref().unwrap()[1].observations.len(), 1);
}
