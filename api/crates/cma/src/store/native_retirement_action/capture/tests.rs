use super::*;
use crate::store::sandbox_maintenance::mongo_integration_tests::{active_sandbox, ready_agent};
use layer2_core::sandbox::SandboxHome;
use layer3_adapter::{HomeVolumePolicy, HomeVolumeReceipt, RetirementPod, RuntimeHandle};
use mongodb::bson::{doc, Bson};

const AT: &str = "2026-09-05T00:00:00Z";

fn fixture() -> (Sandbox, Agent) {
    let mut parent = active_sandbox("sandbox-capture");
    let runtime = parent.runtime.as_mut().unwrap();
    runtime.runtime_id = "agent-sandbox:capture:original".into();
    runtime.boot_id = Some("a".repeat(32));
    let mut home = SandboxHome::reserve(
        &parent.id,
        HomeVolumePolicy {
            mode: Default::default(),
            storage_class: "fixture".into(),
            capacity_gib: 4,
            template_name: "empty".into(),
            template_generation: "fixture".into(),
        },
    )
    .unwrap();
    home.bind(HomeVolumeReceipt {
        native_authority: None,
        volume_id: home.request.volume_id.clone(),
        request_sha256: home.request.computed_sha256(),
        storage_reference: "fixture-home".into(),
        storage_generation: "original".into(),
    })
    .unwrap();
    parent.lifecycle.home = Some(Box::new(home));
    parent.begin_retirement(AT.into()).unwrap();
    let child = ready_agent(&parent, "child");
    (parent, child)
}

#[test]
fn retirement_capture_reoffers_exact_lineage_without_replacing_custody() {
    let (parent, mut child) = fixture();
    let offered = prepare_offer(&doc! {}, &parent, &child).unwrap();
    let row = doc! {CAPTURE: offered.clone()};
    child
        .request_stop(AgentStopReason::SandboxRetired, AT.into())
        .unwrap();
    assert_eq!(prepare_offer(&row, &parent, &child), Ok(offered));
    for field in [FIELD, LEDGER_MAINTENANCE_FIELD] {
        let mut held = row.clone();
        held.insert(field, Bson::Null);
        assert_eq!(prepare_offer(&held, &parent, &child), Err(CONFLICT));
    }
    for changed in [
        Agent {
            worktree_id: WorktreeId("successor".into()),
            ..child.clone()
        },
        Agent {
            workspace_id: WorkspaceId("successor".into()),
            ..child.clone()
        },
        Agent {
            requested_stop_reason: Some(AgentStopReason::Owner),
            ..child.clone()
        },
        Agent {
            owner: SubjectId("other-owner".into()),
            ..child.clone()
        },
        Agent {
            sandbox_id: SandboxId("other-parent".into()),
            ..child.clone()
        },
    ] {
        assert_eq!(prepare_offer(&row, &parent, &changed), Err(CONFLICT));
    }
    child.conversation.runtime_generation = Some(2);
    assert_eq!(prepare_offer(&row, &parent, &child), Err(CONFLICT));
}

#[test]
fn retirement_capture_rejects_superseded_parent_identity() {
    let (parent, child) = fixture();
    let row = doc! {CAPTURE: prepare_offer(&doc! {}, &parent, &child).unwrap()};
    for change in [
        "handle",
        "image",
        "boot",
        "generation",
        "home",
        "owner",
        "step",
    ] {
        let mut next = parent.clone();
        let runtime = next.runtime.as_mut().unwrap();
        match change {
            "handle" => runtime.runtime_id.push_str("-successor"),
            "image" => runtime.runtime_version.push_str("-successor"),
            "boot" => runtime.boot_id = Some("b".repeat(32)),
            "generation" => runtime.provision_generation = Some(2),
            "home" => {
                let home = next.lifecycle.home.as_mut().unwrap();
                home.receipt.as_mut().unwrap().storage_generation = "successor".into();
            }
            "owner" => next.owner = SubjectId("successor".into()),
            "step" => next.work.step = SandboxWorkStep::WaitForPurge,
            _ => unreachable!(),
        }
        let mut current_child = child.clone();
        current_child.owner = next.owner.clone();
        assert_eq!(
            prepare_offer(&row, &next, &current_child),
            Err(CONFLICT),
            "{change}"
        );
    }
}

#[test]
fn retirement_capture_bounds_record_count_and_bson_bytes() {
    let (parent, mut child) = fixture();
    let mut row = doc! {};
    for index in 0..MAX_SANDBOX_LIFECYCLE_AGENTS {
        child.id = AgentId(format!("child-{index}"));
        row = doc! {CAPTURE: prepare_offer(&row, &parent, &child).unwrap()};
    }
    assert!(prepare_offer(&row, &parent, &child).is_ok());
    child.id = AgentId("extra-child".into());
    assert_eq!(prepare_offer(&row, &parent, &child), Err(CONFLICT));

    let document = doc! {"padding": "x".repeat(MAX_BYTES - 19)};
    assert_eq!(bson::serialize_to_vec(&document).unwrap().len(), MAX_BYTES);
    assert!(size_allowed(&document).unwrap());
    assert!(!size_allowed(&doc! {"padding": "x".repeat(MAX_BYTES - 18)}).unwrap());
    child.worktree_id = WorktreeId("x".repeat(MAX_BYTES));
    assert_eq!(prepare_offer(&doc! {}, &parent, &child), Err(CONFLICT));
    assert!(matches!(
        CaptureRecord::decode(&doc! {"padding": "x".repeat(MAX_BYTES)}),
        Err(SandboxStoreError::Corrupt(_))
    ));
}

#[test]
fn retirement_capture_round_trips_bound_and_observed_slots_and_refuses_malformed_records() {
    let (parent, mut child) = fixture();
    let encoded = prepare_offer(&doc! {}, &parent, &child).unwrap();
    let mut record = CaptureRecord::decode(&encoded).unwrap();
    let attempt = BoundAttempt {
        id: Uuid::new_v4(),
        child_revision: 4,
        claim: AgentWorkLease {
            holder: "fixture-worker".into(),
            fence: 3,
            expires_at: AT.into(),
        },
        owner: TransportOwner {
            agent_id: child.id.to_string(),
            instance_id: "fixture-manager".into(),
            protocol: layer3_adapter::owner_protocol::OWNER_PROTOCOL_VERSION,
            epoch: 2,
        },
    };
    child.revision = attempt.child_revision;
    child.work.lease = Some(attempt.claim.clone());
    record.slots[0].state = CaptureState::Bound {
        attempt: attempt.clone(),
    };
    assert_eq!(
        CaptureRecord::decode(&record.encode().unwrap()),
        Ok(record.clone())
    );
    record.slots[0].state = CaptureState::Observed {
        receiver: RetirementReceiver {
            runtime: RuntimeHandle::new(&record.parent.runtime.runtime_id),
            pod: RetirementPod {
                uid: "original-pod".into(),
                container_id: Some("original".into()),
                restart_count: Some(0),
            },
            owner: attempt.owner.clone(),
        },
        attempt,
        refusal: ChallengeRefusal::ServerError,
    };
    let observed = record.encode().unwrap();
    assert_eq!(CaptureRecord::decode(&observed), Ok(record.clone()));
    let row = doc! {CAPTURE: observed.clone()};
    assert_eq!(prepare_offer(&row, &parent, &child), Ok(observed));

    child.revision += 1;
    let replacement = prepare_offer(&row, &parent, &child).unwrap();
    let replacement = CaptureRecord::decode(&replacement).unwrap();
    assert_ne!(replacement.slots[0].offer_id, record.slots[0].offer_id);
    assert_eq!(replacement.slots[0].state, CaptureState::Offered);
    let mut duplicate = record.clone();
    duplicate.slots.push(record.slots[0].clone());
    let CaptureState::Observed { receiver, .. } = &mut record.slots[0].state else {
        unreachable!()
    };
    receiver.owner.epoch += 1;
    assert!(matches!(
        duplicate.encode(),
        Err(SandboxStoreError::Corrupt(_))
    ));
    assert!(matches!(
        record.encode(),
        Err(SandboxStoreError::Corrupt(_))
    ));
    for malformed in [
        Bson::Null,
        Bson::Document(doc! {"version": 2}),
        Bson::Document(bson::serialize_to_document(&duplicate).unwrap()),
        Bson::Document(bson::serialize_to_document(&record).unwrap()),
    ] {
        assert!(matches!(
            prepare_offer(&doc! {CAPTURE: malformed}, &parent, &child),
            Err(SandboxStoreError::Corrupt(_))
        ));
    }
}
