use super::*;

fn digest(value: char) -> wire::Digest {
    wire::Digest::new(&value.to_string().repeat(64)).unwrap()
}
fn generation(value: u64) -> wire::Generation {
    wire::Generation::new(value).unwrap()
}

#[test]
fn grant_capacity_reserves_evidence_and_closing_never_admits() {
    let mut control = GrantControlV1 {
        protocol: 1,
        key: wire::GrantFamilyKey {
            origin: digest('a'),
            share_id: wire::OpaqueId::new("synthetic-share").unwrap(),
            generation: generation(1),
        },
        epoch: generation(1),
        revision: generation(1),
        members: 0,
        state: GrantStateV1::Open,
    };
    for index in 1..=MAX_FAMILY_MEMBERS {
        assert_eq!(control.reserve_member().unwrap(), index);
    }
    assert_eq!(control.reserve_member(), Err(ExecutionError::Capacity));
    let operation = digest('b');
    control.state = GrantStateV1::Closing {
        operation: operation.clone(),
    };
    assert_eq!(
        control.reserve_member(),
        Err(ExecutionError::Closing { operation })
    );
    let mut wrong_generation = control.key.clone();
    wrong_generation.generation = generation(2);
    assert_eq!(
        control.require_open(&wrong_generation, control.epoch),
        Err(ExecutionError::AuthorityChanged)
    );
    assert_eq!(control.members, MAX_FAMILY_MEMBERS);
}

#[test]
fn completion_cannot_omit_a_committed_effect_from_its_journal() {
    let mut record = member(1);
    complete(&mut record, &operation(1));
    record.effect_count = 1;
    assert!(
        record.validate().is_err(),
        "stop evidence omitted a durable effect intent"
    );
}

#[tokio::test]
async fn unsupported_release_port_has_no_capability() {
    let gate = UnsupportedReleaseGate;
    assert!(!gate.supported());
    let release = wire::ReleaseKey {
        response_id: wire::OpaqueId::new("synthetic-response").unwrap(),
        content_fingerprint: digest('a'),
        receiver: wire::ReceiverKey {
            kind: wire::ReceiverKind::ReleaseProducer,
            instance: wire::OpaqueId::new("synthetic-producer").unwrap(),
            incarnation: digest('b'),
            boot: wire::OpaqueId::new("synthetic-boot").unwrap(),
        },
    };
    assert_eq!(
        gate.challenge(&release).await,
        Err(ExecutionError::Disabled)
    );
}

fn id(value: &str) -> wire::OpaqueId {
    wire::OpaqueId::new(value).unwrap()
}

fn ticket() -> wire::ExecutionTicketV1 {
    let receiver = wire::ReceiverKey {
        kind: wire::ReceiverKind::Runtime,
        instance: id("runtime"),
        incarnation: digest('c'),
        boot: id("boot"),
    };
    wire::ExecutionTicketV1 {
        protocol: 1,
        execution: wire::ExecutionKey {
            command_id: id("command"),
            command_fingerprint: digest('d'),
            dispatch_sequence: generation(1),
            prompt_id: id("prompt"),
            turn_id: id("turn"),
            agent_id: id("agent"),
            receiver: receiver.clone(),
            attempt: generation(1),
        },
        basis: wire::ExecutionBasisV1::Share {
            grant: wire::GrantKey {
                family: wire::GrantFamilyKey {
                    origin: digest('a'),
                    share_id: id("share"),
                    generation: generation(1),
                },
                share_revision: generation(1),
                target_ancestry: digest('b'),
                capability: wire::ProtectedCapability::Prompt,
                policy_revision: generation(1),
                actor: digest('e'),
            },
        },
        operation: wire::ExecutionOperation::Prompt,
        input_digest: digest('f'),
        manager: wire::ManagerKey {
            instance: id("manager"),
            epoch: generation(1),
            agent_id: id("agent"),
        },
        challenge: wire::ReceiverChallengeV1 {
            protocol: 1,
            receiver,
            nonce: id("challenge"),
            created_tick_ms: 100,
            deadline_tick_ms: 3_100,
        },
        nonce: id("ticket"),
        registration: digest('0'),
    }
}

fn member(ordinal: u32) -> ExecutionRecordV1 {
    let mut ticket = ticket();
    ticket.execution.command_id = id(&format!("command-{ordinal}"));
    let member = wire::MemberKey::Execution {
        execution: ticket.execution.clone(),
    };
    ticket.registration = registration_id(&member).unwrap();
    ExecutionRecordV1 {
        protocol: 1,
        registration: ticket.registration.clone(),
        epoch: generation(1),
        target: ResourceTarget::Agent {
            agent_id: crate::AgentId("agent".into()),
        },
        basis: ticket.basis.clone(),
        member,
        ordinal,
        permit: Some(MemberPermitV1::Execution { ticket }),
        continuation: None,
        started: None,
        barrier: None,
        completion: None,
        effect_count: 0,
    }
}

fn operation(count: u32) -> RevocationOperationV1 {
    use crate::resource_access::{ResourceOwner, ResourceSelector, ShareChange};
    let mut intent = RevocationIntentV1 {
        selector: ResourceSelector::Sandbox {
            sandbox_id: crate::SandboxId("sandbox".into()),
        },
        owner: ResourceOwner::Person {
            subject: crate::SubjectId::new("owner"),
        },
        actor: ResourceActor::Human {
            subject: crate::SubjectId::new("owner"),
        },
        key_sha256: digest('a'),
        change: ShareChange::Revoke {
            share_id: "share".into(),
        },
        expected_share_revision: 1,
        sandbox_id: crate::SandboxId("sandbox".into()),
        sandbox_created_at: "2026-01-01T00:00:00Z".into(),
        ancestry: digest('b'),
        original_policy_revision: 1,
        proposed_id: id("proposed"),
        requested_at: "2026-01-01T00:00:01Z".into(),
        fingerprint: digest('0'),
    };
    let request = intent.mutation(1);
    intent.fingerprint = wire::Digest::new(&request.fingerprint()).unwrap();
    RevocationOperationV1 {
        protocol: 1,
        key: wire::RevocationKeyV1 {
            operation_id: wire::Digest::new(&request.operation_id()).unwrap(),
            family: ticket().basis.family().unwrap().clone(),
            watermark: generation(2),
        },
        epoch: generation(1),
        intent,
        frozen_members: count,
        erased_ordinals: Default::default(),
        frozen_at_unix_ms: 1_000,
        deadline_unix_ms: 11_000,
        phase: RevocationPhaseV1::Pending,
    }
}

fn complete(member: &mut ExecutionRecordV1, operation: &RevocationOperationV1) {
    let request = member.barrier_request(operation).unwrap();
    let barrier = wire::BarrierReceiptV1 {
        protocol: 1,
        request: request.clone(),
        barrier_sequence: generation(2),
        durable_watermark: generation(2),
    };
    member.observe_barrier(&request, &barrier).unwrap();
    member
        .observe_completion(&MemberCompletionV1::Execution {
            receipt: wire::StopReceiptV1 {
                protocol: 1,
                barrier,
                stop_sequence: generation(3),
                evidence: wire::StopEvidenceKind::RejectedBeforeStart,
                quiescence: wire::QuiescenceV1 {
                    queues_purged: true,
                    descendants_absent: true,
                    output_closed: true,
                    submitters_closed: true,
                    pending_local_effects_settled: true,
                    local_changes_digest: digest('a'),
                    effects: wire::EffectJournalV1 {
                        high_water: 0,
                        digest: digest('b'),
                        unresolved_count: 0,
                    },
                },
            },
        })
        .unwrap();
}

#[test]
fn finalization_requires_every_frozen_member_despite_timeout_or_duplicate_ack() {
    let operation = operation(2);
    let mut one = member(1);
    let mut two = member(2);
    complete(&mut one, &operation);
    assert!(operation.deadline_exceeded_at(100_000).unwrap());
    assert!(operation.validate_cohort(&[one.clone()]).is_err());
    assert!(operation
        .validate_cohort(&[one.clone(), two.clone()])
        .is_err());
    assert!(operation
        .validate_cohort(&[one.clone(), one.clone()])
        .is_err());
    complete(&mut two, &operation);
    complete(&mut two, &operation);
    operation
        .validate_cohort(&[one.clone(), two.clone()])
        .unwrap();
    let mut wrong = two;
    wrong.epoch = generation(2);
    assert!(operation.validate_cohort(&[one, wrong]).is_err());
    let retained: RevocationOperationV1 =
        serde_json::from_slice(&serde_json::to_vec(&operation).unwrap()).unwrap();
    assert_eq!(retained.deadline_unix_ms, 11_000);
    assert!(retained.deadline_exceeded_at(100_000).unwrap());
    assert!(matches!(retained.phase, RevocationPhaseV1::Pending));
}

#[test]
fn entered_execution_cannot_later_claim_rejected_before_start() {
    let mut member = member(1);
    complete(&mut member, &operation(1));
    let Some(MemberPermitV1::Execution { ticket }) = &member.permit else {
        panic!()
    };
    let observation = wire::StartObservationV1 {
        protocol: 1,
        execution: ticket.execution.clone(),
        registration: member.registration.clone(),
        nonce: ticket.nonce.clone(),
        state: wire::NativeStartState::Entered,
        sequence: generation(1),
    };
    assert!(member.observe_start(&observation).is_err());
    assert!(member.started.is_none());
    let mut wrong = member.completion.clone().unwrap();
    let MemberCompletionV1::Execution { receipt } = &mut wrong else {
        panic!()
    };
    receipt.barrier.request.revocation.family.generation = generation(2);
    assert!(member.observe_completion(&wrong).is_err());
}

#[test]
fn unknown_effect_is_evidence_only_until_confirmed() {
    let record = member(1);
    let Some(MemberPermitV1::Execution { ticket }) = &record.permit else {
        panic!()
    };
    let intent = wire::EffectIntentV1 {
        effect_id: id("effect"),
        execution: ticket.execution.clone(),
        registration: record.registration.clone(),
        grant: ticket.basis.clone(),
        operation: wire::EffectOperationClass::ToolRequest,
        request_digest: digest('c'),
        provider_idempotency: None,
    };
    let mut effect = EffectRecordV1 {
        protocol: 1,
        permission: wire::EffectPermissionV1 {
            protocol: 1,
            intent: intent.clone(),
            ledger_sequence: generation(1),
            permission_id: super::digest(&("resource-effect-permission-v1", &intent)).unwrap(),
        },
        observations: Vec::new(),
    };
    assert_eq!(effect.replay_prepared(&intent).unwrap(), effect.permission);
    let mut capacity = effect.clone();
    let mut observation = wire::EffectObservationV1 {
        protocol: 1,
        permission_id: effect.permission.permission_id.clone(),
        execution: intent.execution.clone(),
        effect_id: intent.effect_id.clone(),
        sequence: generation(1),
        outcome: wire::EffectOutcome::SubmissionUnknown,
        evidence_digest: digest('d'),
    };
    effect.observe(&observation).unwrap();
    effect.observe(&observation).unwrap();
    assert_eq!(
        effect.replay_prepared(&intent),
        Err(ExecutionError::Unknown)
    );
    observation.sequence = generation(2);
    observation.outcome = wire::EffectOutcome::CommitConfirmed;
    let uncertain = effect.observations[0].clone();
    effect.observe(&observation).unwrap();
    effect.observe(&uncertain).unwrap();
    assert_eq!(effect.observations, vec![uncertain, observation.clone()]);
    let mut changed = observation.clone();
    changed.evidence_digest = digest('f');
    assert!(effect.observe(&changed).is_err());
    assert_eq!(
        effect.replay_prepared(&intent),
        Err(ExecutionError::Unknown)
    );
    observation.sequence = generation(3);
    observation.outcome = wire::EffectOutcome::SubmissionUnknown;
    assert!(effect.observe(&observation).is_err());
    observation.outcome = wire::EffectOutcome::ReconciliationRequired;
    for sequence in 1..MAX_EFFECT_OBSERVATIONS {
        observation.sequence = generation(sequence as u64);
        capacity.observe(&observation).unwrap();
    }
    observation.sequence = generation(MAX_EFFECT_OBSERVATIONS as u64);
    let before = capacity.clone();
    assert_eq!(
        capacity.observe(&observation),
        Err(ExecutionError::Capacity)
    );
    assert_eq!(capacity, before);
    observation.outcome = wire::EffectOutcome::CommitConfirmed;
    capacity.observe(&observation).unwrap();
    capacity.validate().unwrap();
    let mut invalid = capacity.clone();
    invalid.observations.remove(0);
    assert!(invalid.validate().is_err());
    invalid = capacity.clone();
    invalid.observations.last_mut().unwrap().outcome = wire::EffectOutcome::ReconciliationRequired;
    assert!(invalid.validate().is_err());
    invalid = capacity;
    invalid.observations.push(observation);
    assert!(invalid.validate().is_err());
}

#[test]
fn independent_basis_preserves_provenance_without_reusing_a_share_or_binding_generation() {
    use crate::resource_access::{OrganizationRole, ResourceAuthorizationBasis as B};
    let convert = |basis: &B| {
        leases::independent_basis(
            basis,
            digest('a'),
            digest('b'),
            digest('c'),
            wire::ProtectedCapability::ReadHistory,
        )
        .unwrap()
        .unwrap()
    };
    let personal = convert(&B::PersonalOwner);
    let operator = convert(&B::GlobalOperator);
    let org = B::Organization {
        issuer: "synthetic".into(),
        organization: crate::SubjectId::new("org"),
        role: OrganizationRole::Admin,
        observed_at: "2026-01-01T00:00:00Z".into(),
        expires_at: "2026-01-01T00:01:00Z".into(),
        share: None,
    };
    let organization = convert(&org);
    let binding = convert(&B::ExternalBinding {
        binding_id: "binding".into(),
        generation: 1,
    });
    assert!(matches!(
        personal,
        wire::ExecutionBasisV1::IndependentOwner { .. }
    ));
    for basis in [&operator, &organization, &binding] {
        assert!(matches!(basis, wire::ExecutionBasisV1::Independent { .. }));
        assert!(basis.family().is_none());
        basis.validate().unwrap();
        let value = serde_json::to_value(basis).unwrap();
        assert_eq!(
            *basis,
            serde_json::from_value::<wire::ExecutionBasisV1>(value).unwrap()
        );
    }
    assert_ne!(operator, organization);
    assert_ne!(binding, organization);
    assert_ne!(
        binding,
        convert(&B::ExternalBinding {
            binding_id: "binding".into(),
            generation: 2
        })
    );
    let mut refreshed = org.clone();
    if let B::Organization {
        observed_at,
        expires_at,
        ..
    } = &mut refreshed
    {
        *observed_at = "2026-01-01T00:00:01Z".into();
        *expires_at = "2026-01-01T00:01:01Z".into();
    }
    assert_eq!(organization, convert(&refreshed));
    if let B::Organization { share, .. } = &mut refreshed {
        *share = Some(crate::resource_access::ResourceGrantObservation {
            id: "share".into(),
            revision: 1,
            mode: crate::resource_access::ShareMode::View,
            selector: crate::resource_access::ResourceSelector::Sandbox {
                sandbox_id: crate::SandboxId("sandbox".into()),
            },
        });
    }
    assert_eq!(
        leases::independent_basis(
            &refreshed,
            digest('a'),
            digest('b'),
            digest('c'),
            wire::ProtectedCapability::ReadHistory
        )
        .unwrap(),
        None
    );
}

// #3296: erasure recomputes exactly the digests the writers store.
#[test]
fn subject_execution_digests_match_the_stored_actor_and_owner_digests() {
    let subject = crate::SubjectId::new("acct:erased");
    let launch = crate::LaunchId("lch_erased".into());
    let values = subject_execution_digests(&subject, std::slice::from_ref(&launch)).unwrap();
    let human = ResourceActor::Human {
        subject: subject.clone(),
    };
    let trigger = ResourceActor::Trigger {
        subject: subject.clone(),
        launch_id: launch,
    };
    let person = super::super::ResourceOwner::Person {
        subject: subject.clone(),
    };
    for expected in [
        actor_digest(&human).unwrap().as_str().to_owned(),
        actor_digest(&trigger).unwrap().as_str().to_owned(),
        super::digest(&person).unwrap().as_str().to_owned(),
    ] {
        assert!(values.contains(&expected));
    }
    assert_eq!(values.len(), 5);
    let other = subject_execution_digests(&crate::SubjectId::new("acct:other"), &[]).unwrap();
    assert!(other.iter().all(|value| !values.contains(value)));
}
