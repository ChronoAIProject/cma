use super::*;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};

#[path = "model_credit_test_authority.rs"]
mod authority;

pub(in crate::sandbox) use authority::{BeginReply, Ledger};

#[path = "model_credit_dispatch_tests.rs"]
mod dispatch;

pub(in crate::sandbox) fn units(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}

pub(in crate::sandbox) fn payer(selected: CredentialBinding) -> PayerBinding {
    let name = match selected.source {
        CredentialSource::Managed => "managed",
        CredentialSource::Own => "own",
    };
    PayerBinding {
        credential: selected,
        payer: name.into(),
        pool: "shared-pool".into(),
    }
}

pub(in crate::sandbox) fn credential(source: CredentialSource) -> CredentialBinding {
    CredentialBinding {
        source,
        generation: units(1),
        route: "synthetic-model".into(),
    }
}

#[derive(Default)]
struct Gateway {
    certification_unavailable: AtomicBool,
    ambiguous_reply: AtomicBool,
    effect_lookup_unavailable: AtomicBool,
    change_binding_on_dispatch: AtomicBool,
    work: Mutex<HashMap<PhaseId, Effect>>,
    requests: AtomicUsize,
}

#[async_trait]
impl FundedModelGateway for Gateway {
    type Request = u64;

    async fn certify(&self, selected: &CredentialBinding) -> Result<PayerBinding, FundingError> {
        if self.certification_unavailable.load(Ordering::SeqCst) {
            return Err(FundingError::Unavailable);
        }
        if selected.route != "synthetic-model" {
            return Err(FundingError::Unsupported);
        }
        Ok(payer(selected.clone()))
    }

    fn digest(&self, request: &u64) -> [u8; 32] {
        Sha256::digest(request.to_be_bytes()).into()
    }

    async fn execute(&self, permit: &Checkpoint, request: u64) -> Result<Effect, FundingError> {
        if self.change_binding_on_dispatch.load(Ordering::SeqCst)
            || request > permit.reservation.limit.get()
        {
            return Ok(Effect::NotStarted);
        }
        let mut work = self.work.lock().unwrap();
        assert!(!work.contains_key(&permit.reservation.id));
        let effect = Effect::Finished { spent: request };
        self.requests.fetch_add(1, Ordering::SeqCst);
        work.insert(permit.reservation.id.clone(), effect);
        if self.ambiguous_reply.load(Ordering::SeqCst) {
            return Err(FundingError::ReconciliationRequired);
        }
        Ok(effect)
    }

    async fn reconcile(&self, permit: &Checkpoint) -> Result<Effect, FundingError> {
        if self.effect_lookup_unavailable.load(Ordering::SeqCst) {
            return Err(FundingError::ReconciliationRequired);
        }
        self.work
            .lock()
            .unwrap()
            .get(&permit.reservation.id)
            .copied()
            .ok_or(FundingError::ReconciliationRequired)
    }
}

struct Owner;
#[async_trait]
impl ContinuationAuthority for Owner {
    async fn authorize(&self, caller: &crate::Caller, _: &Checkpoint) -> Result<(), FundingError> {
        if caller.subject.as_str() == "turn-owner" {
            Ok(())
        } else {
            Err(FundingError::Forbidden)
        }
    }
}

#[tokio::test]
async fn replicas_compete_for_the_same_pool_without_funding_own_credentials_from_it() {
    let ledger = Ledger::default();
    let gateway = Gateway::default();
    let managed = credential(CredentialSource::Managed);
    let own = credential(CredentialSource::Own);
    ledger.lock().unwrap().add(&payer(managed.clone()), 10);
    let first = ModelCredit::new(&ledger, &gateway);
    let second = ModelCredit::new(&ledger, &gateway);
    let (a, b) = tokio::join!(
        first.reserve("turn-one".into(), &managed, units(10), &7),
        second.reserve("turn-two".into(), &managed, units(10), &7),
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1);
    assert_eq!(
        a.clone().err().or(b.clone().err()),
        Some(FundingError::Insufficient)
    );
    assert_eq!(
        first.reserve("own-turn".into(), &own, units(1), &1).await,
        Err(FundingError::Insufficient)
    );
    let held = a.or(b).unwrap();
    let id = &held.reservation.id;
    let (a, b) = tokio::join!(
        first.dispatch(id, &managed, 7),
        second.dispatch(id, &managed, 7)
    );
    assert!(a.or(b).is_ok());
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);
    assert_eq!(*ledger.lock().unwrap().available(&payer(managed)), 3);
}

#[tokio::test]
async fn credential_route_input_and_limit_are_rechecked_before_billable_dispatch() {
    let ledger = Ledger::default();
    let gateway = Gateway::default();
    let managed = credential(CredentialSource::Managed);
    ledger.lock().unwrap().add(&payer(managed.clone()), 10);
    let credit = ModelCredit::new(&ledger, &gateway);
    gateway
        .certification_unavailable
        .store(true, Ordering::SeqCst);
    assert_eq!(
        credit
            .reserve("no-authority".into(), &managed, units(5), &5)
            .await,
        Err(FundingError::Unavailable)
    );
    gateway
        .certification_unavailable
        .store(false, Ordering::SeqCst);
    let held = credit
        .reserve("turn".into(), &managed, units(5), &6)
        .await
        .unwrap();
    let id = &held.reservation.id;
    let rotated = CredentialBinding {
        generation: units(2),
        ..managed.clone()
    };
    let unavailable = CredentialBinding {
        route: "unsupported-route".into(),
        ..managed.clone()
    };
    assert_eq!(
        credit.dispatch(id, &rotated, 6).await,
        Err(FundingError::BindingChanged)
    );
    assert_eq!(
        credit.dispatch(id, &unavailable, 6).await,
        Err(FundingError::Unsupported)
    );
    assert_eq!(
        credit.dispatch(id, &managed, 5).await,
        Err(FundingError::CheckpointChanged)
    );
    let settled = credit.dispatch(id, &managed, 6).await.unwrap();
    assert_eq!(settled.state, PhaseState::Settled(Effect::NotStarted));
    let held = credit
        .reserve("new-turn".into(), &managed, units(3), &3)
        .await
        .unwrap();
    gateway
        .change_binding_on_dispatch
        .store(true, Ordering::SeqCst);
    let settled = credit
        .dispatch(&held.reservation.id, &managed, 3)
        .await
        .unwrap();
    assert_eq!(settled.state, PhaseState::Settled(Effect::NotStarted));
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(*ledger.lock().unwrap().available(&payer(managed)), 10);
}

#[tokio::test]
async fn expired_reservation_cannot_dispatch_or_imply_settlement() {
    let ledger = Ledger::default();
    let gateway = Gateway::default();
    let managed = credential(CredentialSource::Managed);
    ledger.lock().unwrap().add(&payer(managed.clone()), 10);
    let credit = ModelCredit::new(&ledger, &gateway);
    let held = credit
        .reserve("expiring-turn".into(), &managed, units(10), &7)
        .await
        .unwrap();
    let id = &held.reservation.id;
    ledger.lock().unwrap().expire(id.clone());
    assert_eq!(
        credit.dispatch(id, &managed, 7).await,
        Err(FundingError::Expired)
    );
    assert_eq!(ledger.read(id).await.unwrap(), held);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(*ledger.lock().unwrap().available(&payer(managed)), 0);
}

#[tokio::test]
async fn lost_reply_holds_the_charge_until_reconciled_and_top_up_resumes_only_once() {
    let ledger = Ledger::default();
    let gateway = Gateway::default();
    let managed = credential(CredentialSource::Managed);
    let binding = payer(managed.clone());
    let owner = crate::Caller::new(
        crate::SubjectId("turn-owner".into()),
        crate::Authorization::none(),
    );
    let foreign = crate::Caller::new(
        crate::SubjectId("another-owner".into()),
        crate::Authorization::none(),
    );
    ledger.lock().unwrap().add(&binding, 5);
    let credit = ModelCredit::new(&ledger, &gateway);
    let held = credit
        .reserve("turn".into(), &managed, units(5), &5)
        .await
        .unwrap();
    gateway.ambiguous_reply.store(true, Ordering::SeqCst);
    gateway
        .effect_lookup_unavailable
        .store(true, Ordering::SeqCst);
    let id = &held.reservation.id;
    assert_eq!(
        credit.dispatch(id, &managed, 5).await,
        Err(FundingError::ReconciliationRequired)
    );
    let resumed_replica = ModelCredit::new(&ledger, &gateway);
    assert_eq!(
        resumed_replica.reconcile(id).await,
        Err(FundingError::ReconciliationRequired)
    );
    assert_eq!(
        resumed_replica.dispatch(id, &managed, 5).await,
        Err(FundingError::ReconciliationRequired)
    );
    let pending = ledger.read(id).await.unwrap();
    assert_eq!(
        resumed_replica
            .continue_after(&Owner, &owner, &pending, &managed, units(3), &3)
            .await,
        Err(FundingError::ReconciliationRequired)
    );
    gateway
        .effect_lookup_unavailable
        .store(false, Ordering::SeqCst);
    let (a, b) = tokio::join!(credit.reconcile(id), resumed_replica.reconcile(id));
    let settled = a.unwrap();
    assert_eq!(b.unwrap(), settled);
    assert_eq!(
        ledger
            .settle(&pending, Effect::Finished { spent: 5 })
            .await
            .unwrap(),
        settled
    );
    assert_eq!(
        resumed_replica
            .continue_after(&Owner, &owner, &settled, &managed, units(3), &3)
            .await,
        Err(FundingError::Insufficient)
    );
    ledger.lock().unwrap().add(&binding, 10);
    assert_eq!(
        resumed_replica
            .continue_after(&Owner, &foreign, &settled, &managed, units(3), &3)
            .await,
        Err(FundingError::Forbidden)
    );
    let (a, b) = tokio::join!(
        credit.continue_after(&Owner, &owner, &settled, &managed, units(3), &3),
        resumed_replica.continue_after(&Owner, &owner, &settled, &managed, units(3), &3),
    );
    assert_eq!(a.is_ok() as u8 + b.is_ok() as u8, 1);
    assert_eq!(
        a.clone().err().or(b.clone().err()),
        Some(FundingError::CheckpointChanged)
    );
    let next = a.or(b).unwrap();
    gateway.ambiguous_reply.store(false, Ordering::SeqCst);
    assert_eq!(
        credit
            .dispatch(&next.reservation.id, &managed, 3)
            .await
            .unwrap()
            .state,
        PhaseState::Settled(Effect::Finished { spent: 3 })
    );
    assert_eq!(*ledger.lock().unwrap().available(&binding), 7);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 2);
}
