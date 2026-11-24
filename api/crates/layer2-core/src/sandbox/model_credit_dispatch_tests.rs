use super::*;

async fn assert_uncertain_begin(reply: BeginReply) {
    let ledger = Ledger::default();
    let gateway = Gateway::default();
    let selected = credential(CredentialSource::Managed);
    let binding = payer(selected.clone());
    ledger.lock().unwrap().add(&binding, 5);
    let credit = ModelCredit::new(&ledger, &gateway);
    let held = credit
        .reserve("uncertain-begin".into(), &selected, units(5), &3)
        .await
        .unwrap();
    let id = &held.reservation.id;
    ledger.lock().unwrap().begin_reply = Some(reply);
    let result = credit.dispatch(id, &selected, 3).await;
    let pending = ledger.read(id).await.unwrap();
    assert_eq!(pending.state, PhaseState::InFlight);
    assert_eq!(pending.reservation, held.reservation);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
    assert_eq!(
        result,
        Err(FundingError::ReconciliationRequired),
        "{reply:?}"
    );

    let successor = ModelCredit::new(&ledger, &gateway);
    assert_eq!(
        successor.dispatch(id, &selected, 3).await,
        Err(FundingError::ReconciliationRequired)
    );
    assert_eq!(
        successor.reconcile(id).await,
        Err(FundingError::ReconciliationRequired)
    );
    assert_eq!(ledger.read(id).await.unwrap(), pending);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 0);
    assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
}

#[tokio::test]
async fn uncertain_begin_errors_require_reconciliation_without_dispatch_or_refund() {
    for error in [
        FundingError::Unavailable,
        FundingError::CheckpointChanged,
        FundingError::NoReservation,
    ] {
        assert_uncertain_begin(BeginReply::Error(error)).await;
    }
}

#[tokio::test]
async fn malformed_begin_permits_require_reconciliation_without_dispatch_or_refund() {
    for reply in [
        BeginReply::UnchangedVersion,
        BeginReply::ChangedReservation,
        BeginReply::WrongState,
    ] {
        assert_uncertain_begin(reply).await;
    }
}

#[tokio::test]
async fn concurrent_begin_loser_preserves_the_winners_unconfirmed_effect() {
    let ledger = Ledger::default();
    let gateway = Gateway::default();
    let selected = credential(CredentialSource::Managed);
    let binding = payer(selected.clone());
    ledger.lock().unwrap().add(&binding, 5);
    let first = ModelCredit::new(&ledger, &gateway);
    let second = ModelCredit::new(&ledger, &gateway);
    let held = first
        .reserve("competing-begin".into(), &selected, units(5), &3)
        .await
        .unwrap();
    ledger.lock().unwrap().begin_barrier = Some(std::sync::Arc::new(tokio::sync::Barrier::new(2)));
    gateway.ambiguous_reply.store(true, Ordering::SeqCst);
    let id = &held.reservation.id;
    let (left, right) = tokio::join!(
        first.dispatch(id, &selected, 3),
        second.dispatch(id, &selected, 3),
    );
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.read(id).await.unwrap().state, PhaseState::InFlight);
    assert_eq!(*ledger.lock().unwrap().available(&binding), 0);
    assert_eq!(left, Err(FundingError::ReconciliationRequired));
    assert_eq!(right, Err(FundingError::ReconciliationRequired));
    let settled = first.reconcile(id).await.unwrap();
    assert_eq!(
        settled.state,
        PhaseState::Settled(Effect::Finished { spent: 3 })
    );
    assert_eq!(second.reconcile(id).await.unwrap(), settled);
    assert_eq!(second.dispatch(id, &selected, 3).await.unwrap(), settled);
    assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);
    assert_eq!(*ledger.lock().unwrap().available(&binding), 2);
}

#[tokio::test]
async fn in_flight_dispatch_keeps_original_funding_when_credential_certification_changes() {
    for certification_unavailable in [false, true] {
        let ledger = Ledger::default();
        let gateway = Gateway::default();
        let selected = credential(CredentialSource::Managed);
        let binding = payer(selected.clone());
        ledger.lock().unwrap().add(&binding, 5);
        let credit = ModelCredit::new(&ledger, &gateway);
        let held = credit
            .reserve("retained-funding".into(), &selected, units(5), &3)
            .await
            .unwrap();
        let id = &held.reservation.id;
        gateway.ambiguous_reply.store(true, Ordering::SeqCst);
        assert_eq!(
            credit.dispatch(id, &selected, 3).await,
            Err(FundingError::ReconciliationRequired)
        );
        let pending = ledger.read(id).await.unwrap();
        let rotated = CredentialBinding {
            generation: units(2),
            ..selected
        };
        gateway
            .certification_unavailable
            .store(certification_unavailable, Ordering::SeqCst);
        assert_eq!(
            credit.dispatch(id, &rotated, 3).await,
            Err(FundingError::ReconciliationRequired)
        );
        assert_eq!(
            credit.dispatch(id, &rotated, 4).await,
            Err(FundingError::CheckpointChanged)
        );
        assert_eq!(ledger.read(id).await.unwrap(), pending);
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);
        assert_eq!(*ledger.lock().unwrap().available(&binding), 0);

        let settled = credit.reconcile(id).await.unwrap();
        assert_eq!(settled.reservation.binding, binding);
        assert_eq!(
            settled.state,
            PhaseState::Settled(Effect::Finished { spent: 3 })
        );
        assert_eq!(credit.reconcile(id).await.unwrap(), settled);
        assert_eq!(gateway.requests.load(Ordering::SeqCst), 1);
        assert_eq!(*ledger.lock().unwrap().available(&binding), 2);
    }
}
