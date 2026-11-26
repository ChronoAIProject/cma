use super::*;
use crate::resource_access::{ResourceActor, ResourceTarget};

pub(super) fn digest(value: char) -> wire::Digest {
    wire::Digest::new(&value.to_string().repeat(64)).unwrap()
}
pub(super) fn generation(value: u64) -> wire::Generation {
    wire::Generation::new(value).unwrap()
}

pub(super) fn id(value: &str) -> wire::OpaqueId {
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

pub(super) fn member(ordinal: u32) -> ExecutionRecordV1 {
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

pub(super) fn operation(count: u32) -> RevocationOperationV1 {
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
        frozen_at_unix_ms: 1_000,
        deadline_unix_ms: 11_000,
        phase: RevocationPhaseV1::Pending,
    }
}

pub(super) fn final_receipt(
    op: &RevocationOperationV1,
) -> crate::resource_access::ShareMutationReceipt {
    use crate::resource_access::*;
    let request = op.intent.mutation(op.intent.original_policy_revision);
    ShareMutationReceipt {
        operation_id: request.operation_id(),
        command_fingerprint: request.fingerprint(),
        sandbox_id: op.intent.sandbox_id.clone(),
        sandbox_created_at: op.intent.sandbox_created_at.clone(),
        share: ResourceShare {
            id: op.key.family.share_id.as_str().into(),
            owner: op.intent.owner.clone(),
            selector: op.intent.selector.clone(),
            target_org_id: crate::SubjectId::new("organization"),
            link: None,
            mode: ShareMode::OrgRoles,
            issuer_actor: op.intent.actor.clone(),
            created_at: op.intent.sandbox_created_at.clone(),
            revoked_at: Some(op.intent.requested_at.clone()),
            revision: 2,
        },
        actor: op.intent.actor.clone(),
        at: op.intent.requested_at.clone(),
        policy_revision: 2,
    }
}

pub(super) fn complete(row: &mut ExecutionRecordV1, op: &RevocationOperationV1) {
    let barrier = wire::BarrierReceiptV1 {
        protocol: 1,
        request: row.barrier_request(op).unwrap(),
        barrier_sequence: generation(2),
        durable_watermark: op.key.watermark,
    };
    let receipt = wire::StopReceiptV1 {
        protocol: 1,
        barrier: barrier.clone(),
        stop_sequence: generation(3),
        evidence: wire::StopEvidenceKind::RejectedBeforeStart,
        quiescence: wire::QuiescenceV1 {
            queues_purged: true,
            descendants_absent: true,
            output_closed: true,
            submitters_closed: true,
            pending_local_effects_settled: true,
            local_changes_digest: digest('f'),
            effects: wire::EffectJournalV1 {
                high_water: u64::from(row.effect_count),
                digest: digest('e'),
                unresolved_count: row.effect_count,
            },
        },
    };
    row.observe_barrier(&barrier.request, &barrier).unwrap();
    row.observe_completion(&MemberCompletionV1::Execution { receipt })
        .unwrap();
}
