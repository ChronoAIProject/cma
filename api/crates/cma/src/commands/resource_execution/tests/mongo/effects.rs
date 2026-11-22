use super::super::super::reconciliation::EffectReconciler;
use super::super::reconciliation::fixture::Provider;
use super::*;
use wire::EffectOutcome as Outcome;

pub(super) async fn prepare(fixture: &Fixture) -> EffectRecordV1 {
    let authority = fixture
        .authority("recipient", ResourceCapability::Interact)
        .await;
    let execution = fixture.execution("external-effect");
    let record = fixture
        .coordinator
        .register_execution(&authority, &execution)
        .await
        .unwrap();
    let Some(MemberPermitV1::Execution { mut ticket }) = member(1).permit else {
        panic!()
    };
    ticket.execution = execution.clone();
    ticket.basis = authority.basis().clone();
    ticket.registration = record.registration;
    ticket.manager.agent_id = execution.agent_id.clone();
    ticket.challenge.receiver = execution.receiver.clone();
    fixture
        .coordinator
        .issue_ticket(&authority, &ticket)
        .await
        .unwrap();
    fixture
        .coordinator
        .observe_start(
            &execution.receiver,
            &wire::StartObservationV1 {
                protocol: 1,
                execution: execution.clone(),
                registration: ticket.registration.clone(),
                nonce: ticket.nonce.clone(),
                state: wire::NativeStartState::Entered,
                sequence: generation(1),
            },
        )
        .await
        .unwrap();
    let intent = wire::EffectIntentV1 {
        effect_id: id("external-effect"),
        execution: execution.clone(),
        registration: ticket.registration,
        grant: ticket.basis,
        operation: wire::EffectOperationClass::ToolRequest,
        request_digest: digest('e'),
        provider_idempotency: Some(digest('f')),
    };
    let permission = fixture
        .coordinator
        .prepare_effect(&execution.receiver, &intent)
        .await
        .unwrap();
    let observation = wire::EffectObservationV1 {
        protocol: 1,
        permission_id: permission.permission_id.clone(),
        execution,
        effect_id: permission.intent.effect_id.clone(),
        sequence: generation(1),
        outcome: Outcome::SubmissionUnknown,
        evidence_digest: digest('a'),
    };
    fixture.store.observe_effect(&observation).await.unwrap();
    EffectRecordV1 {
        protocol: 1,
        permission,
        observations: vec![observation],
    }
}

async fn finish(fixture: &Fixture, effect: &EffectRecordV1) -> RevocationOperationV1 {
    let operation = fixture.freeze().await;
    complete(fixture, effect, &operation).await
}

pub(super) async fn complete(
    fixture: &Fixture,
    effect: &EffectRecordV1,
    operation: &RevocationOperationV1,
) -> RevocationOperationV1 {
    let row = fixture
        .store
        .record(&effect.permission.intent.registration)
        .await
        .unwrap()
        .unwrap();
    let directory = Directory::new(std::slice::from_ref(&row));
    let endpoint = directory.endpoint(1);
    let mut completion = endpoint.completion(&row.barrier_request(operation).unwrap());
    let MemberCompletionV1::Execution { receipt } = &mut completion else {
        panic!()
    };
    receipt.evidence = wire::StopEvidenceKind::CooperativeQuiescence;
    *endpoint.retained.lock().unwrap() = Some(completion);
    let worker = RevocationWorker::new(fixture.store.clone(), directory);
    assert_eq!(
        worker.reconcile(&operation.key.operation_id).await.unwrap(),
        Progress::Effective
    );
    final_operation(fixture, operation).await
}

// Provider replies and stop receipts are synthetic; transactions and retained
// effect/revocation records are real. This does not qualify a live provider.
#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn submitted_effect_remains_reconcilable_after_effective_and_store_reopen() {
    let fixture = Fixture::new().await;
    let effect = prepare(&fixture).await;
    let provider = Provider::new(&effect, Outcome::CommitConfirmed);
    let confirmed = provider.reply.lock().unwrap().clone();
    *provider.reply.lock().unwrap() = Ok(None);
    let registration = &effect.permission.intent.registration;
    let before = EffectReconciler::new(fixture.store.clone(), provider.clone());
    assert_eq!(before.reconcile(registration).await.unwrap().unresolved, 1);
    let final_receipt = finish(&fixture, &effect).await;
    assert!(matches!(
        fixture
            .coordinator
            .prepare_effect(
                &effect.permission.intent.execution.receiver,
                &effect.permission.intent
            )
            .await,
        Err(ExecutionError::Closing { .. }) | Err(ExecutionError::AuthorityChanged)
    ));
    let store = fixture.reopened().await;
    let recovered = EffectReconciler::new(store.clone(), provider.clone());
    assert_eq!(
        recovered.reconcile(registration).await.unwrap().unresolved,
        1
    );
    *provider.reply.lock().unwrap() = confirmed;
    assert_eq!(recovered.reconcile(registration).await.unwrap().terminal, 1);
    assert_eq!(recovered.reconcile(registration).await.unwrap().terminal, 1);
    assert_eq!(
        *provider.calls.lock().unwrap(),
        vec![effect.permission.intent.clone(); 3]
    );
    let records = store.effects_for(registration, 0, 32).await.unwrap();
    assert_eq!(records[0].permission, effect.permission);
    assert_eq!(records[0].observations.len(), 2);
    assert_eq!(records[0].observations[0], effect.observations[0]);
    assert_eq!(records[0].observations[1].outcome, Outcome::CommitConfirmed);
    assert!(
        store
            .revocation(&final_receipt.key.operation_id)
            .await
            .unwrap()
            .unwrap()
            == final_receipt
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn competing_effect_status_workers_converge_without_duplicate_outcomes() {
    let fixture = Fixture::new().await;
    let effect = prepare(&fixture).await;
    finish(&fixture, &effect).await;
    let provider = Provider::new(&effect, Outcome::Rejected);
    *provider.gate.lock().unwrap() = Some(Arc::new(tokio::sync::Barrier::new(2)));
    let first = EffectReconciler::new(fixture.store.clone(), provider.clone());
    let second = EffectReconciler::new(fixture.reopened().await, provider.clone());
    let registration = &effect.permission.intent.registration;
    let (a, b) = tokio::join!(
        first.reconcile(registration),
        second.reconcile(registration)
    );
    assert_eq!(a.unwrap().terminal, 1);
    assert_eq!(b.unwrap().terminal, 1);
    let records = fixture
        .store
        .effects_for(registration, 0, 32)
        .await
        .unwrap();
    assert_eq!(records[0].observations.len(), 2);
    assert_eq!(records[0].observations[1].outcome, Outcome::Rejected);
    assert_eq!(provider.calls.lock().unwrap().len(), 2);
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a replica set"]
async fn uncertain_effect_append_reloads_evidence_after_manager_replacement() {
    let fixture = Fixture::new().await;
    let effect = prepare(&fixture).await;
    finish(&fixture, &effect).await;
    let provider = Provider::new(&effect, Outcome::CommitConfirmed);
    let first = EffectReconciler::new(fixture.store.clone(), provider.clone());
    let registration = &effect.permission.intent.registration;
    fixture.ambiguous_commits(true).await;
    let uncertain = first.reconcile(registration).await.unwrap();
    fixture.ambiguous_commits(false).await;
    assert_eq!(uncertain.terminal, 0);
    assert_eq!(
        uncertain.failures,
        vec![(
            effect.permission.intent.effect_id.clone(),
            ExecutionError::Unknown
        )]
    );
    let replacement = EffectReconciler::new(fixture.reopened().await, provider.clone());
    assert_eq!(
        replacement.reconcile(registration).await.unwrap().terminal,
        1
    );
    assert_eq!(provider.calls.lock().unwrap().len(), 1);
    let records = fixture
        .store
        .effects_for(registration, 0, 32)
        .await
        .unwrap();
    assert_eq!(records[0].observations.len(), 2);
    fixture.cleanup().await;
}
