use super::*;
use mongodb::bson::DateTime;

mod mongo;

fn target(symbol: char, family: &str) -> ReleaseTarget {
    let digest = symbol.to_string().repeat(64);
    ReleaseTarget {
        image: format!("registry.example/{family}@sha256:{digest}"),
        plan_sha256: digest.clone(),
        target_id: format!("wp_{digest}"),
        target_revision: 1,
        pool_name: format!("pool-{family}-{symbol}"),
        pool_uid: format!("pool-uid-{family}-{symbol}"),
        template_name: format!("template-{family}-{symbol}"),
        template_uid: format!("template-uid-{family}-{symbol}"),
        qualification_sha256: digest,
    }
}

fn pair(agent: char, machine: char) -> ReleasePair {
    ReleasePair {
        agent: target(agent, "agent"),
        machine: Some(target(machine, "machine")),
    }
}

fn writer(epoch: i64, fence: i64) -> WriterIdentity {
    WriterIdentity {
        deployment: "test-deployment".into(),
        instance_id: format!("instance-{epoch}"),
        config_sha256: "a".repeat(64),
        epoch,
        fence,
    }
}

fn operator() -> OperatorIdentity {
    OperatorIdentity {
        subject: "operator".into(),
        authorization_sha256: "b".repeat(64),
    }
}

fn bootstrap() -> BootstrapRequest {
    BootstrapRequest {
        deployment: "test-deployment".into(),
        operation_id: "bootstrap".into(),
        operator: operator(),
        pair: pair('a', 'b'),
        fallback_pair: pair('c', 'd'),
        writer: writer(1, 1),
    }
}

fn head() -> ReleaseHead {
    let b = bootstrap();
    ReleaseHead {
        deployment: b.deployment,
        schema_version: 1,
        epoch: 1,
        control_revision: 1,
        admission_sequence: 0,
        active_effects: 0,
        phase: Phase::Active,
        pair: b.pair.clone(),
        writer: Some(b.writer),
        writer_fence: 1,
        operation_id: None,
        protected_pairs: vec![b.pair, b.fallback_pair],
        committed_at: DateTime::from_millis(1),
    }
}

fn transition(head: &ReleaseHead) -> TransitionRequest {
    TransitionRequest {
        deployment: head.deployment.clone(),
        operation_id: "restore".into(),
        operator: operator(),
        expected_epoch: head.epoch,
        expected_control_revision: head.control_revision,
        before: head.pair.clone(),
        after: bootstrap().fallback_pair,
        next_writer: writer(head.epoch + 1, head.writer_fence + 1),
    }
}

fn operation(request: &TransitionRequest) -> ReleaseOperation {
    let original = OperationRequest::Transition(Box::new(request.clone()));
    let digest = validation::hash(&original).unwrap();
    ReleaseOperation {
        key: validation::key(&request.deployment, &request.operation_id).unwrap(),
        schema_version: 1,
        deployment: request.deployment.clone(),
        operation_id: request.operation_id.clone(),
        request_sha256: digest.clone(),
        request: original,
        predecessor_operation_id: None,
        receipts: vec![ControlReceipt {
            request_sha256: digest,
            evidence_sha256: "a".repeat(64),
            epoch: request.expected_epoch,
            control_revision: request.expected_control_revision + 1,
            phase: Phase::Freezing,
        }],
    }
}

fn step(head: &ReleaseHead, action: AdvanceAction) -> AdvanceRequest {
    AdvanceRequest {
        deployment: head.deployment.clone(),
        operation_id: "restore".into(),
        operator: operator(),
        expected_epoch: head.epoch,
        expected_control_revision: head.control_revision,
        expected_phase: head.phase,
        action,
    }
}

struct TestEvidence;

#[async_trait::async_trait]
impl EvidenceVerifier for TestEvidence {
    async fn verify(&self, request: &EvidenceRequest) -> Result<VerifiedEvidence> {
        // Store mechanics fixture only; never a production finality collector.
        VerifiedEvidence::from_verified_observations(request, validation::hash(request)?)
    }
}

#[test]
fn freeze_preserves_inflight_effects_and_refuses_drain_until_they_settle() {
    let mut h = head();
    h.admission_sequence = 7;
    h.active_effects = 2;
    let request = transition(&h);
    let frozen = transitions::begin(&h, &request).unwrap();
    assert_eq!(frozen.active_effects, 2);
    assert_eq!(frozen.admission_sequence, 7);
    assert_eq!(
        transitions::advance(
            &frozen,
            &operation(&request),
            &step(&frozen, AdvanceAction::ConfirmDrained)
        ),
        Err(AuthorityError::EvidenceRequired)
    );
    assert!(
        transitions::advance(
            &frozen,
            &operation(&request),
            &step(&frozen, AdvanceAction::Hold)
        )
        .is_ok()
    );
}

#[test]
fn commit_changes_both_families_once_and_revokes_old_writer() {
    let original = head();
    let request = transition(&original);
    let op = operation(&request);
    let mut h = transitions::begin(&original, &request).unwrap();
    for action in [
        AdvanceAction::ConfirmDrained,
        AdvanceAction::Prepare,
        AdvanceAction::Commit,
    ] {
        h = transitions::advance(&h, &op, &step(&h, action)).unwrap();
    }
    assert_eq!(h.epoch, 2);
    assert_eq!(h.pair, request.after);
    assert_eq!(h.writer, None);
    assert_eq!(h.protected_pairs, original.protected_pairs);
    assert_eq!(
        transitions::advance(&h, &op, &step(&h, AdvanceAction::Abort)),
        Err(AuthorityError::Conflict)
    );
    assert_eq!(
        transitions::advance(&h, &op, &step(&h, AdvanceAction::Commit)),
        Err(AuthorityError::Conflict)
    );
    h = transitions::advance(&h, &op, &step(&h, AdvanceAction::Activate)).unwrap();
    assert_eq!(h.writer, Some(writer(2, 2)));
    assert_eq!(h.phase, Phase::Active);
    assert_eq!(h.operation_id, None);
}

#[test]
fn stale_and_unprotected_target_requests_refuse_without_head_mutation() {
    let h = head();
    let mut r = transition(&h);
    r.expected_control_revision += 1;
    assert_eq!(transitions::begin(&h, &r), Err(AuthorityError::Conflict));
    r = transition(&h);
    r.after = pair('e', 'f');
    assert_eq!(transitions::begin(&h, &r), Err(AuthorityError::Conflict));
    r = transition(&h);
    r.next_writer.instance_id = writer(1, 1).instance_id;
    assert_eq!(transitions::begin(&h, &r), Err(AuthorityError::Conflict));
    assert_eq!(h, head());
}

#[test]
fn precommit_abort_keeps_release_and_writer_but_does_not_skip_drain() {
    let initial = head();
    let request = transition(&initial);
    let op = operation(&request);
    let mut h = transitions::begin(&initial, &request).unwrap();
    h.admission_sequence = 1;
    h.active_effects = 1;
    assert_eq!(
        transitions::advance(&h, &op, &step(&h, AdvanceAction::Abort)),
        Err(AuthorityError::EvidenceRequired)
    );
    h.active_effects = 0;
    let next = transitions::advance(&h, &op, &step(&h, AdvanceAction::Abort)).unwrap();
    assert_eq!(next.pair, initial.pair);
    assert_eq!(next.writer, initial.writer);
    assert_eq!(next.epoch, initial.epoch);
    assert_eq!(next.phase, Phase::Active);
}

#[test]
fn aborting_reversal_restores_failed_hold_without_reviving_a_writer() {
    let initial = head();
    let first = transition(&initial);
    let first_op = operation(&first);
    let mut h = transitions::begin(&initial, &first).unwrap();
    for action in [
        AdvanceAction::ConfirmDrained,
        AdvanceAction::Prepare,
        AdvanceAction::Commit,
        AdvanceAction::Hold,
    ] {
        h = transitions::advance(&h, &first_op, &step(&h, action)).unwrap();
    }
    let held = h.clone();
    let mut reversal = transition(&h);
    reversal.operation_id = "reverse".into();
    reversal.after = initial.pair;
    let mut reverse_op = operation(&reversal);
    reverse_op.predecessor_operation_id = held.operation_id.clone();
    h = transitions::begin(&h, &reversal).unwrap();
    for action in [AdvanceAction::Hold, AdvanceAction::Abort] {
        let mut request = step(&h, action);
        request.operation_id = reversal.operation_id.clone();
        h = transitions::advance(&h, &reverse_op, &request).unwrap();
    }
    assert_eq!(h.phase, Phase::FailedHold);
    assert_eq!(h.writer, None);
    assert_eq!(h.operation_id, held.operation_id);
    assert_eq!(h.pair, held.pair);
    assert_eq!(h.epoch, held.epoch);
    assert!(
        transitions::begin(
            &h,
            &TransitionRequest {
                expected_control_revision: h.control_revision,
                ..reversal
            }
        )
        .is_ok()
    );
    // The original committed operation still needs fresh activation evidence.
    let resumed = transitions::advance(&h, &first_op, &step(&h, AdvanceAction::Activate)).unwrap();
    assert_eq!(resumed.writer, Some(first.next_writer));
}

#[test]
fn malformed_family_or_generation_is_not_a_valid_authority() {
    let mut p = pair('a', 'b');
    p.machine = Some(p.agent.clone());
    assert_eq!(p.validate(), Err(AuthorityError::Invalid));
    let mut h = head();
    h.active_effects = 1;
    assert_eq!(h.validate(), Err(AuthorityError::Invalid));
    h = head();
    h.writer.as_mut().unwrap().epoch += 1;
    assert_eq!(h.validate(), Err(AuthorityError::Invalid));
    h = head();
    h.control_revision = i64::MAX;
    assert_eq!(
        transitions::begin(&h, &transition(&h)),
        Err(AuthorityError::Conflict)
    );
}

#[test]
fn proof_is_bound_to_exact_action_revision_and_subject() {
    let request = EvidenceRequest {
        deployment: "test-deployment".into(),
        operation_id: "bootstrap".into(),
        epoch: 0,
        control_revision: 0,
        action: EvidenceAction::Bootstrap,
        request_sha256: "a".repeat(64),
        state_sha256: "b".repeat(64),
        subject: EvidenceSubject::Bootstrap {
            request: bootstrap(),
        },
    };
    let proof = VerifiedEvidence::from_verified_observations(&request, "c".repeat(64)).unwrap();
    let mut changed = request.clone();
    changed.control_revision += 1;
    assert_eq!(
        proof.clone().check(&changed),
        Err(AuthorityError::EvidenceRequired)
    );
    changed = request.clone();
    changed.action = EvidenceAction::Commit;
    assert_eq!(proof.check(&changed), Err(AuthorityError::EvidenceRequired));
}
