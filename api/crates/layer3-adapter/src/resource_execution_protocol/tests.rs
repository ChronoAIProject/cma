use super::*;

fn id(value: &str) -> OpaqueId {
    OpaqueId::new(value).unwrap()
}
fn digest(value: char) -> Digest {
    Digest::new(&value.to_string().repeat(64)).unwrap()
}
fn generation(value: u64) -> Generation {
    Generation::new(value).unwrap()
}

fn receiver() -> ReceiverKey {
    ReceiverKey {
        kind: ReceiverKind::Runtime,
        instance: id("runtime"),
        incarnation: digest('a'),
        boot: id("boot"),
    }
}

fn ticket() -> ExecutionTicketV1 {
    ExecutionTicketV1 {
        protocol: PROTOCOL_VERSION,
        execution: ExecutionKey {
            command_id: id("command"),
            command_fingerprint: digest('b'),
            dispatch_sequence: generation(1),
            prompt_id: id("prompt"),
            turn_id: id("turn"),
            agent_id: id("agent"),
            receiver: receiver(),
            attempt: generation(1),
        },
        basis: ExecutionBasisV1::Share {
            grant: GrantKey {
                family: GrantFamilyKey {
                    origin: digest('c'),
                    share_id: id("share"),
                    generation: generation(1),
                },
                share_revision: generation(2),
                target_ancestry: digest('d'),
                capability: ProtectedCapability::Prompt,
                policy_revision: generation(3),
                actor: digest('e'),
            },
        },
        operation: ExecutionOperation::Prompt,
        input_digest: digest('f'),
        manager: ManagerKey {
            instance: id("manager"),
            epoch: generation(1),
            agent_id: id("agent"),
        },
        challenge: ReceiverChallengeV1 {
            protocol: 1,
            receiver: receiver(),
            nonce: id("challenge"),
            created_tick_ms: 50,
            deadline_tick_ms: 3_050,
        },
        nonce: id("ticket"),
        registration: digest('1'),
    }
}

fn barrier() -> BarrierRequestV1 {
    let ticket = ticket();
    BarrierRequestV1 {
        protocol: 1,
        revocation: RevocationKeyV1 {
            operation_id: digest('2'),
            family: ticket.basis.family().unwrap().clone(),
            watermark: generation(2),
        },
        member: MemberKey::Execution {
            execution: ticket.execution,
        },
        registration: ticket.registration,
        reason: StopReason::ShareRevoked,
    }
}

fn stop() -> StopReceiptV1 {
    StopReceiptV1 {
        protocol: 1,
        barrier: BarrierReceiptV1 {
            protocol: 1,
            request: barrier(),
            barrier_sequence: generation(2),
            durable_watermark: generation(2),
        },
        stop_sequence: generation(3),
        evidence: StopEvidenceKind::TargetedTermination,
        quiescence: QuiescenceV1 {
            queues_purged: true,
            descendants_absent: true,
            output_closed: true,
            submitters_closed: true,
            pending_local_effects_settled: true,
            local_changes_digest: digest('3'),
            effects: EffectJournalV1 {
                high_water: 2,
                digest: digest('4'),
                unresolved_count: 1,
            },
        },
    }
}

#[test]
fn identifiers_are_bounded_and_diagnostics_do_not_echo_input() {
    for bad in [
        "".to_owned(),
        " ".into(),
        "a".repeat(129),
        "line\nbreak".into(),
    ] {
        assert!(OpaqueId::new(&bad).is_err());
        assert!(serde_json::from_str::<OpaqueId>(&serde_json::to_string(&bad).unwrap()).is_err());
    }
    assert!(OpaqueId::new(&"a".repeat(128)).is_ok());
    for bad in [
        "A".repeat(64),
        "g".repeat(64),
        "a".repeat(63),
        "a".repeat(65),
    ] {
        assert!(Digest::new(&bad).is_err());
        assert!(serde_json::from_str::<Digest>(&serde_json::to_string(&bad).unwrap()).is_err());
    }
    for bad in [0, i64::MAX as u64 + 1, u64::MAX] {
        assert!(Generation::new(bad).is_err());
        assert!(serde_json::from_str::<Generation>(&bad.to_string()).is_err());
    }
    assert!(generation(i64::MAX as u64).successor().is_err());
    let wire = ticket();
    let debug = format!("{wire:?} {:?}", stop());
    for private in [
        "\"command\"",
        "\"runtime\"",
        "\"share\"",
        "\"manager\"",
        &"a".repeat(64),
    ] {
        assert!(!debug.contains(private));
    }
}

#[test]
fn wire_roundtrip_rejects_unknown_fields_and_versions() {
    let value = ticket();
    let bytes = serde_json::to_vec(&value).unwrap();
    let decoded: ExecutionTicketV1 = decode_message(&bytes).unwrap();
    assert_eq!(decoded, value);
    decoded.validate().unwrap();
    let mut object = serde_json::to_value(&value).unwrap();
    object["bearer"] = "unexpected".into();
    assert!(decode_message::<ExecutionTicketV1>(&serde_json::to_vec(&object).unwrap()).is_err());
    assert!(decode_message::<ExecutionTicketV1>(&vec![b' '; MAX_MESSAGE_BYTES + 1]).is_err());
    let mut wrong = value;
    wrong.protocol = 2;
    assert_eq!(wrong.validate(), Err(ProtocolError::Unsupported));
    let mut wrong = ticket();
    wrong.challenge.protocol = 2;
    assert_eq!(wrong.validate(), Err(ProtocolError::Unsupported));
    let decoded: StopReceiptV1 = decode_message(&serde_json::to_vec(&stop()).unwrap()).unwrap();
    decoded.validate_for(&barrier()).unwrap();
}

#[test]
fn delayed_challenge_reply_cannot_extend_or_reboot_the_deadline() {
    let challenge = ticket().challenge;
    challenge.check_at(&receiver(), 3_049).unwrap();
    for tick in [49, 3_050, 100_000] {
        assert_eq!(
            challenge.check_at(&receiver(), tick),
            Err(ProtocolError::Expired)
        );
    }
    let mut rebooted = receiver();
    rebooted.boot = id("successor-boot");
    assert_eq!(
        challenge.check_at(&rebooted, 50),
        Err(ProtocolError::AudienceMismatch)
    );
    for (created, deadline) in [(50, 50), (51, 50), (50, 3_051), (u64::MAX, 10)] {
        let mut bad = challenge.clone();
        bad.created_tick_ms = created;
        bad.deadline_tick_ms = deadline;
        assert!(bad.validate().is_err());
    }
}

#[test]
fn ticket_matches_exact_owner_execution_input_and_receiver() {
    let value = ticket();
    let owner = crate::owner_protocol::TransportOwner {
        protocol: 1,
        agent_id: "agent".into(),
        instance_id: "manager".into(),
        epoch: 1,
    };
    value
        .validate_for(&owner, &value.execution, &value.input_digest, 100)
        .unwrap();
    let mut wrong_owner = owner.clone();
    wrong_owner.epoch += 1;
    assert!(value
        .validate_for(&wrong_owner, &value.execution, &value.input_digest, 100)
        .is_err());
    let mut wrong_execution = value.execution.clone();
    wrong_execution.attempt = generation(2);
    assert!(value
        .validate_for(&owner, &wrong_execution, &value.input_digest, 100)
        .is_err());
    assert!(value
        .validate_for(&owner, &value.execution, &digest('0'), 100)
        .is_err());
    let mut wrong = value.clone();
    wrong.execution.receiver.kind = ReceiverKind::ReleaseProducer;
    assert!(wrong.validate().is_err());
    let mut wrong = value;
    let ExecutionBasisV1::Share { grant } = &mut wrong.basis else {
        unreachable!()
    };
    grant.share_revision = generation(4);
    assert!(wrong.validate().is_err());
}

#[test]
fn family_is_stable_across_actor_target_capability_and_policy_observations() {
    let original = ticket();
    let mut changed = original.clone();
    let ExecutionBasisV1::Share { grant } = &mut changed.basis else {
        unreachable!()
    };
    grant.actor = digest('0');
    grant.target_ancestry = digest('1');
    grant.capability = ProtectedCapability::ReadHistory;
    grant.share_revision = generation(5);
    grant.policy_revision = generation(6);
    assert_eq!(changed.basis.family(), original.basis.family());
    assert_ne!(changed.basis, original.basis);
    let ExecutionBasisV1::Share { grant } = &mut changed.basis else {
        unreachable!()
    };
    grant.family.generation = generation(2);
    assert_ne!(changed.basis.family(), original.basis.family());
}

#[test]
fn continuation_never_substitutes_a_basis_or_reuses_a_start_challenge() {
    let original = ticket();
    let mut lease = ContinuationV1 {
        protocol: 1,
        execution: original.execution.clone(),
        registration: original.registration.clone(),
        basis: original.basis.clone(),
        manager: original.manager.clone(),
        challenge: original.challenge.clone(),
        sequence: generation(2),
    };
    assert!(lease.validate_for(&original).is_err());
    lease.challenge.nonce = id("continuation-challenge");
    lease.challenge.created_tick_ms += 1_000;
    lease.challenge.deadline_tick_ms += 1_000;
    lease.validate_for(&original).unwrap();
    let ExecutionBasisV1::Share { grant } = &mut lease.basis else {
        unreachable!()
    };
    grant.family.generation = generation(2);
    assert!(lease.validate_for(&original).is_err());
}

#[test]
fn ticket_cannot_promote_read_authority_into_native_control() {
    for operation in [
        ExecutionOperation::Prompt,
        ExecutionOperation::RecoverablePrompt,
        ExecutionOperation::Steer,
        ExecutionOperation::PermissionAnswer,
        ExecutionOperation::Configure,
        ExecutionOperation::ContextChange,
    ] {
        let mut value = ticket();
        value.operation = operation;
        let ExecutionBasisV1::Share { grant } = &mut value.basis else {
            unreachable!()
        };
        grant.capability = ProtectedCapability::ReadHistory;
        assert_eq!(value.validate(), Err(ProtocolError::EvidenceMismatch));
    }
}

#[test]
fn continuation_must_advance_the_original_challenge_and_sequence() {
    let original = ticket();
    for (created, deadline, sequence) in [(49, 3_049, 2), (50, 3_050, 2), (1_050, 4_050, 1)] {
        let value = ContinuationV1 {
            protocol: 1,
            execution: original.execution.clone(),
            registration: original.registration.clone(),
            basis: original.basis.clone(),
            manager: original.manager.clone(),
            challenge: ReceiverChallengeV1 {
                nonce: id("different"),
                created_tick_ms: created,
                deadline_tick_ms: deadline,
                ..original.challenge.clone()
            },
            sequence: generation(sequence),
        };
        assert_eq!(
            value.validate_for(&original),
            Err(ProtocolError::EvidenceMismatch)
        );
    }
}

#[test]
fn stop_requires_exact_frozen_member_and_positive_durable_quiescence() {
    let good = stop();
    good.validate_for(&barrier()).unwrap();
    for index in 0..5 {
        let mut bad = good.clone();
        match index {
            0 => bad.quiescence.queues_purged = false,
            1 => bad.quiescence.descendants_absent = false,
            2 => bad.quiescence.output_closed = false,
            3 => bad.quiescence.submitters_closed = false,
            _ => bad.quiescence.pending_local_effects_settled = false,
        }
        assert!(bad.validate_for(&barrier()).is_err());
    }
    for index in 0..7 {
        let mut wrong = barrier();
        match index {
            0 => wrong.revocation.operation_id = digest('0'),
            1 => wrong.revocation.family.generation = generation(3),
            2 => wrong.revocation.family.origin = digest('0'),
            3 => wrong.registration = digest('0'),
            _ => {
                let MemberKey::Execution { execution } = &mut wrong.member else {
                    unreachable!()
                };
                match index {
                    4 => execution.receiver.boot = id("new-boot"),
                    5 => execution.receiver.incarnation = digest('0'),
                    _ => execution.turn_id = id("new-turn"),
                }
            }
        }
        assert!(good.validate_for(&wrong).is_err());
    }
    let mut bad = good.clone();
    bad.stop_sequence = bad.barrier.barrier_sequence;
    assert!(bad.validate_for(&barrier()).is_err());
    let mut bad = good;
    bad.quiescence.effects.unresolved_count = 3;
    assert!(bad.validate_for(&barrier()).is_err());
}

#[tokio::test]
async fn unsupported_control_never_fabricates_capability_or_ack() {
    let control = UnsupportedExecutionControl;
    assert!(control.capabilities().is_none());
    assert_eq!(
        control.challenge(&ticket().execution).await,
        Err(ProtocolError::Unsupported)
    );
    assert_eq!(
        control.admit_start(&ticket()).await,
        Err(ProtocolError::Unsupported)
    );
    assert_eq!(
        control.install_barrier(&barrier()).await,
        Err(ProtocolError::Unsupported)
    );
    assert_eq!(
        control.cancel(&barrier()).await,
        Err(ProtocolError::Unsupported)
    );
    assert_eq!(
        control.stop_receipt(&barrier()).await,
        Err(ProtocolError::Unsupported)
    );
}

#[test]
fn release_closure_requires_the_exact_producer_and_all_pending_bytes_closed() {
    let mut receiver = receiver();
    receiver.kind = ReceiverKind::ReleaseProducer;
    let release = ReleaseKey {
        response_id: id("response"),
        content_fingerprint: digest('a'),
        receiver,
    };
    let mut request = barrier();
    request.member = MemberKey::Release { release };
    let receipt = ReleaseReceiptV1 {
        protocol: 1,
        barrier: BarrierReceiptV1 {
            protocol: 1,
            request: request.clone(),
            barrier_sequence: generation(2),
            durable_watermark: request.revocation.watermark,
        },
        close_sequence: generation(3),
        producers_closed: true,
        pending_chunks_discarded: true,
        transport_drained: true,
        released_bytes: 42,
    };
    receipt.validate_for(&request).unwrap();
    for index in 0..4 {
        let mut bad = receipt.clone();
        match index {
            0 => bad.producers_closed = false,
            1 => bad.pending_chunks_discarded = false,
            2 => bad.transport_drained = false,
            _ => bad.close_sequence = generation(2),
        }
        assert!(bad.validate_for(&request).is_err());
    }
    assert!(receipt.validate_for(&barrier()).is_err());
    assert!(stop().validate_for(&request).is_err());
}

#[test]
fn effect_permission_and_start_observation_cannot_cross_execution_identity() {
    let ticket = ticket();
    let intent = EffectIntentV1 {
        effect_id: id("effect"),
        execution: ticket.execution.clone(),
        registration: ticket.registration.clone(),
        grant: ticket.basis.clone(),
        operation: EffectOperationClass::ToolRequest,
        request_digest: digest('a'),
        provider_idempotency: Some(digest('b')),
    };
    let permission = EffectPermissionV1 {
        protocol: 1,
        intent: intent.clone(),
        ledger_sequence: generation(1),
        permission_id: digest('c'),
    };
    permission.validate_for(&intent, &ticket).unwrap();
    let mut wrong = intent.clone();
    wrong.request_digest = digest('d');
    assert!(permission.validate_for(&wrong, &ticket).is_err());
    wrong = intent;
    wrong.execution.receiver.boot = id("successor");
    assert!(wrong.validate_for(&ticket).is_err());
    let start = StartObservationV1 {
        protocol: 1,
        execution: ticket.execution.clone(),
        registration: ticket.registration.clone(),
        nonce: ticket.nonce.clone(),
        state: NativeStartState::Entered,
        sequence: generation(1),
    };
    start.validate_for(&ticket).unwrap();
    let mut wrong = ticket;
    wrong.execution.turn_id = id("different-turn");
    assert!(start.validate_for(&wrong).is_err());
}
