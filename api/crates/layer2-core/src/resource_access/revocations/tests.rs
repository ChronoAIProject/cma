use super::*;
use std::sync::Mutex;
#[path = "fixtures.rs"]
mod fixtures;
#[path = "snapshot.rs"]
mod snapshot;
#[path = "store.rs"]
mod store;
#[path = "timeouts.rs"]
mod timeouts;
use fixtures::*;
use store::setup;
struct State {
    operation: RevocationOperationV1,
    snapshot: ResourceAccessSnapshot,
    members: Vec<ExecutionRecordV1>,
    effects: Vec<EffectRecordV1>,
    pages: Vec<u32>,
    missing_page: bool,
    change_owner: bool,
    reads: Vec<Read>,
    gates: Vec<(Read, usize, Arc<tokio::sync::Semaphore>)>,
}
struct Store(Mutex<State>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    Snapshot,
    Operation,
    Members(u32),
    Effects(u32),
}

impl Store {
    async fn before_read(&self, point: Read) {
        let gate = {
            let mut state = self.0.lock().unwrap();
            state.reads.push(point);
            let occurrence = state.reads.iter().filter(|read| **read == point).count();
            state.gates.iter().find_map(|(at, nth, gate)| {
                (*at == point && *nth == occurrence).then(|| gate.clone())
            })
        };
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
    }
}
fn context(subject: &str) -> CallerContext {
    CallerContext {
        credential_source: CredentialSource::Retained,
        caller: crate::Caller::new(
            crate::SubjectId::new(subject),
            crate::Authorization::new([crate::Capability::UseOwnSandboxes]),
        ),
        credential: CredentialContext {
            issuer: "synthetic".into(),
            subject: crate::SubjectId::new(subject),
            credential_id: "credential".into(),
        },
        actor: ResourceActor::Human {
            subject: crate::SubjectId::new(subject),
        },
        memberships: vec![],
    }
}
async fn status(
    service: &ResourceRevocationService,
    store: &Store,
    subject: &str,
) -> ExecutionResult<RevocationStatus> {
    let op = store.0.lock().unwrap().operation.clone();
    service
        .status(
            &context(subject),
            None,
            &op.intent.selector,
            op.key.operation_id.as_str(),
        )
        .await
}
#[tokio::test]
async fn pending_evidence_and_effective_are_distinct_and_read_only() {
    let (store, service) = setup(64).await;
    let value = status(&service, &store, "owner").await.unwrap();
    assert_eq!(value.stage, RevocationStage::Pending);
    assert_eq!((value.participants, value.stopped), (64, 0));
    assert!(value.deadline_exceeded);
    assert!(value.receipt.is_none());
    {
        let mut s = store.0.lock().unwrap();
        assert_eq!(s.pages, [0, 7, 14, 21, 28, 35, 42, 49, 56, 63]);
        let op = s.operation.clone();
        for row in &mut s.members {
            complete(row, &op);
        }
    }
    let value = status(&service, &store, "owner").await.unwrap();
    assert_eq!(value.stage, RevocationStage::EvidenceReceived);
    assert!(value.receipt.is_none());
    {
        let mut s = store.0.lock().unwrap();
        s.operation.phase = RevocationPhaseV1::Effective {
            receipt: Box::new(final_receipt(&s.operation)),
        };
    }
    let value = status(&service, &store, "owner").await.unwrap();
    assert_eq!(value.stage, RevocationStage::Effective);
    assert!(value.receipt.is_some());
    assert!(!value.deadline_exceeded);
}
#[tokio::test]
async fn status_denies_foreign_owner_expired_credential_and_changed_incarnation() {
    let (store, service) = setup(1).await;
    assert!(matches!(
        status(&service, &store, "collaborator").await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert!(store.0.lock().unwrap().pages.is_empty());
    let op = store.0.lock().unwrap().operation.clone();
    let mut expired = context("owner");
    expired.credential_source = CredentialSource::RequestBoundRead {
        expires_at: std::time::SystemTime::UNIX_EPOCH,
    };
    assert!(service
        .status(
            &expired,
            None,
            &op.intent.selector,
            op.key.operation_id.as_str()
        )
        .await
        .is_err());
    let other = ResourceSelector::Sandbox {
        sandbox_id: crate::SandboxId("other".into()),
    };
    assert!(service
        .status(
            &context("owner"),
            None,
            &other,
            op.key.operation_id.as_str()
        )
        .await
        .is_err());
    store.0.lock().unwrap().snapshot.lineage.sandbox.created_at = "2026-01-02T00:00:00Z".into();
    assert!(status(&service, &store, "owner").await.is_err());
}
#[tokio::test]
async fn status_rechecks_owner_after_evidence_io() {
    let (store, service) = setup(1).await;
    store.0.lock().unwrap().change_owner = true;
    assert!(matches!(
        status(&service, &store, "owner").await,
        Err(ExecutionError::AuthorityChanged)
    ));
    assert_eq!(store.0.lock().unwrap().pages, [0]);
}
#[tokio::test]
async fn missing_duplicate_wrong_epoch_and_foreign_completed_evidence_fail_closed() {
    for fault in 0..4 {
        let (store, service) = setup(2).await;
        {
            let mut s = store.0.lock().unwrap();
            match fault {
                0 => s.missing_page = true,
                1 => s.members[1] = s.members[0].clone(),
                2 => s.members[0].epoch = generation(2),
                _ => {
                    let mut other = s.operation.clone();
                    other.key.operation_id = fixtures::digest('b');
                    complete(&mut s.members[0], &other);
                }
            }
        }
        assert!(
            status(&service, &store, "owner").await.is_err(),
            "fault {fault}"
        );
    }
}
#[tokio::test]
async fn empty_cohort_needs_final_receipt_and_effective_cannot_lack_completion() {
    let (store, service) = setup(0).await;
    assert_eq!(
        status(&service, &store, "owner").await.unwrap().stage,
        RevocationStage::EvidenceReceived
    );
    let (store, service) = setup(1).await;
    {
        let mut s = store.0.lock().unwrap();
        s.operation.phase = RevocationPhaseV1::Effective {
            receipt: Box::new(final_receipt(&s.operation)),
        };
    }
    assert!(status(&service, &store, "owner").await.is_err());
}

#[tokio::test]
async fn effective_status_retains_unknown_external_effect_until_observed_terminal() {
    let (store, service) = setup(1).await;
    let mut effect = {
        let mut s = store.0.lock().unwrap();
        let row = &mut s.members[0];
        row.effect_count = 1;
        let wire::MemberKey::Execution { execution } = &row.member else {
            panic!()
        };
        let intent = wire::EffectIntentV1 {
            effect_id: id("effect"),
            execution: execution.clone(),
            registration: row.registration.clone(),
            grant: row.basis.clone(),
            operation: wire::EffectOperationClass::ToolRequest,
            request_digest: fixtures::digest('c'),
            provider_idempotency: None,
        };
        EffectRecordV1 {
            protocol: 1,
            permission: wire::EffectPermissionV1 {
                protocol: 1,
                ledger_sequence: generation(1),
                permission_id: execution::digest(&("resource-effect-permission-v1", &intent))
                    .unwrap(),
                intent,
            },
            observations: vec![],
        }
    };
    {
        let mut s = store.0.lock().unwrap();
        s.effects.push(effect.clone());
        let op = s.operation.clone();
        complete(&mut s.members[0], &op);
        s.operation.phase = RevocationPhaseV1::Effective {
            receipt: Box::new(final_receipt(&op)),
        };
    }
    let value = status(&service, &store, "owner").await.unwrap();
    assert_eq!(value.stage, RevocationStage::Effective);
    assert_eq!(value.unresolved_effects, 1);
    effect
        .observe(&wire::EffectObservationV1 {
            protocol: 1,
            permission_id: effect.permission.permission_id.clone(),
            execution: effect.permission.intent.execution.clone(),
            effect_id: effect.permission.intent.effect_id.clone(),
            sequence: generation(1),
            outcome: wire::EffectOutcome::CommitConfirmed,
            evidence_digest: fixtures::digest('d'),
        })
        .unwrap();
    store.0.lock().unwrap().effects = vec![effect];
    assert_eq!(
        status(&service, &store, "owner")
            .await
            .unwrap()
            .unresolved_effects,
        0
    );
    store.0.lock().unwrap().effects.clear();
    assert!(
        status(&service, &store, "owner").await.is_err(),
        "missing effect must not become confirmed"
    );
}
