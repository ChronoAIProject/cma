use super::*;

#[test]
fn data_pressure_refuses_the_outer_future_before_source_calls() {
    let f = Fixture::new(3600);
    let available = f.policy.budget(IngressClass::Data).available_bytes();
    let pressure = f
        .policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![0u8; available - sizing::shared_owner_bytes::<Vec<u8>>()]);
    assert!(matches!(
        f.operation(false, None),
        Err(AdmittedError::Refused(AdmissionRefusal::AtCapacity { .. }))
    ));
    assert_eq!(f.store.get.calls(), 0);
    assert_eq!(f.native.calls(), 0);
    drop(pressure);
}

#[test]
fn pressure_refuses_holder_cache_candidate_and_decoder_before_effects() {
    let f = Fixture::new(1);
    let port = Admitted {
        manager: &f.manager,
        owner: &f.owner,
        policy: f.policy.clone(),
    };
    let record = Retained(f.store.record().unwrap());
    let cache = Retained(f.native.renewed.lock().unwrap().clone());
    let available = f.policy.budget(IngressClass::Data).available_bytes();
    let pressure = f
        .policy
        .admit(IngressClass::Data, plan(available))
        .unwrap()
        .initialize(|| vec![0u8; available - sizing::shared_owner_bytes::<Vec<u8>>()]);
    for result in [
        port.claim(&record, Duration::from_secs(30)).map(|_| ()),
        port.retain_cache(&record).map(|_| ()),
        port.decode_cache(&cache).map(|_| ()),
        port.finish(&record, CacheUpdate::Replace(&cache))
            .map(|_| ()),
    ] {
        assert!(matches!(
            result,
            Err(Error::Source(AdmittedError::Refused(
                AdmissionRefusal::AtCapacity { .. }
            )))
        ));
    }
    assert_eq!(f.store.claim.calls(), 0);
    assert_eq!(f.store.finish.calls(), 0);
    drop(pressure);
}

#[tokio::test]
async fn final_tokens_keep_the_attachment_after_inputs_and_source_drop() {
    let (root, process) = policy();
    let attachment = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let f = Fixture::with_policy(root.clone(), attachment.policy().clone(), 3600);
    let result = f.operation(false, None).unwrap().await.unwrap();
    let tokens = result.as_ref().unwrap().clone();
    assert!(result.shares_custody_domain(attachment.policy()));
    assert!(tokens.shares_custody_domain(attachment.policy()));
    drop((result, f, attachment));
    assert!(process.admit_attachment().is_err());
    let final_clone = tokens.clone();
    drop(tokens);
    assert!(process.admit_attachment().is_err());
    assert_eq!(final_clone.account_id, "account");
    drop(final_clone);
    drop(process.admit_attachment().unwrap());
    drop(second);
}

#[tokio::test]
async fn diagnostic_keeps_original_custody_after_operation_and_source_drop() {
    let (root, process) = policy();
    let attachment = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let f = Fixture::with_policy(root.clone(), attachment.policy().clone(), 3600);
    let diagnostic = own(&f.policy, || {
        layer3_adapter::AdapterError::Unavailable("synthetic retained failure".into())
    });
    f.store.get.config.lock().unwrap().fault =
        Some(Fault::Source(AdmittedError::Adapter(diagnostic), false));
    let error = f.tokens(false, None).await.unwrap_err();
    let last = error.clone();
    drop((error, f, attachment));
    assert!(process.admit_attachment().is_err());
    assert!(matches!(last, AdmittedError::Adapter(_)));
    drop(last);
    drop(process.admit_attachment().unwrap());
    drop(second);
}

#[tokio::test]
async fn cancellation_at_each_source_boundary_keeps_committed_authority() {
    for stage in 0..3 {
        let f = Fixture::new(1);
        let boundary = match stage {
            0 => &f.store.get,
            1 => &f.native.boundary,
            _ => &f.store.finish,
        };
        boundary.config.lock().unwrap().wait = true;
        let baseline = f.root.available_bytes();
        let mut operation = f.operation(false, None).unwrap();
        assert!(futures_util::poll!(&mut operation).is_pending());
        assert_eq!(boundary.active(), 1);
        drop(operation);
        assert_eq!(boundary.active(), 0);
        assert_eq!(boundary.calls(), 1);
        assert_eq!(
            f.store.record().unwrap().refresh_lease.is_some(),
            stage != 0
        );
        assert_eq!(f.store.finish.calls(), usize::from(stage == 2));
        if stage == 0 {
            assert_eq!(f.root.available_bytes(), baseline);
        }
    }
}

#[tokio::test]
async fn whole_operation_deadline_covers_read_native_and_completion_waits() {
    let fixtures = [Fixture::new(1), Fixture::new(1), Fixture::new(1)];
    fixtures[0].store.get.config.lock().unwrap().wait = true;
    fixtures[1].native.boundary.config.lock().unwrap().wait = true;
    fixtures[2].store.finish.config.lock().unwrap().wait = true;
    let started = std::time::Instant::now();
    let results =
        futures_util::future::join_all(fixtures.iter().map(|f| f.tokens(false, None))).await;
    assert!(started.elapsed() >= REFRESH_BUDGET);
    assert!(started.elapsed() < REFRESH_BUDGET + Duration::from_secs(10));
    for (i, result) in results.into_iter().enumerate() {
        assert_eq!(result.unwrap().unwrap_err(), SubscriptionError::Unavailable);
        let f = &fixtures[i];
        assert_eq!(
            f.store.get.active() + f.native.boundary.active() + f.store.finish.active(),
            0
        );
        assert_eq!(f.store.record().unwrap().refresh_lease.is_some(), i != 0);
        assert_eq!(f.store.finish.calls(), usize::from(i == 2));
    }
}
