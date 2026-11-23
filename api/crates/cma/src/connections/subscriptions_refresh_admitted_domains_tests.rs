use super::*;

fn refused<T: std::fmt::Debug>(result: Result<T, AdmittedError>) {
    assert!(
        matches!(
            result,
            Err(AdmittedError::Refused(AdmissionRefusal::InvalidPlan { .. }))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn input_domains_require_exact_root_and_optional_attachment() {
    let (root, process) = policy();
    let first = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let (_, foreign) = policy();
    let policies = [
        process,
        first.policy().clone(),
        second.policy().clone(),
        foreign,
    ];
    for (receiving, policy) in policies.iter().enumerate() {
        let f = Fixture::with_policy(root.clone(), policy.clone(), 3600);
        for (source, input_policy) in policies.iter().enumerate() {
            let owner = own(input_policy, || SubjectId::new("owner"));
            let previous = own_string(input_policy, "account");
            if source == receiving {
                f.manager
                    .tokens_admitted(&owner, false, Some(&previous), policy)
                    .unwrap()
                    .await
                    .unwrap()
                    .as_ref()
                    .unwrap();
            } else {
                let calls = f.store.get.calls();
                refused(f.manager.tokens_admitted(&owner, false, None, policy));
                refused(
                    f.manager
                        .tokens_admitted(&f.owner, false, Some(&previous), policy),
                );
                assert_eq!(f.store.get.calls(), calls);
            }
        }
    }
}

#[tokio::test]
async fn all_source_operations_refuse_wrong_domain_before_polling() {
    let (root, process) = policy();
    let first = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let (_, foreign) = policy();
    let policies = [
        process,
        first.policy().clone(),
        second.policy().clone(),
        foreign,
    ];
    for (receiving, policy) in policies.iter().enumerate() {
        for (source, wrong) in policies.iter().enumerate() {
            if receiving == source {
                continue;
            }
            for stage in 0..4 {
                let f = Fixture::with_policy(root.clone(), policy.clone(), 1);
                let boundary = match stage {
                    0 => &f.store.get,
                    1 => &f.store.claim,
                    2 => &f.native.boundary,
                    _ => &f.store.finish,
                };
                boundary.config.lock().unwrap().operation = Some(wrong.clone());
                refused(f.tokens(false, None).await);
                assert_eq!(boundary.calls(), 0);
            }
        }
    }
}

#[tokio::test]
async fn returned_envelopes_are_checked_even_for_none_and_domain_errors() {
    let (root, process) = policy();
    let first = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let (_, foreign) = policy();
    let policies = [
        process,
        first.policy().clone(),
        second.policy().clone(),
        foreign,
    ];
    for (receiving, policy) in policies.iter().enumerate() {
        for (source, wrong) in policies.iter().enumerate() {
            if receiving == source {
                continue;
            }
            for stage in 0..4 {
                for empty in 0..3 {
                    let f = Fixture::with_policy(root.clone(), policy.clone(), 1);
                    let boundary = match stage {
                        0 => &f.store.get,
                        1 => &f.store.claim,
                        2 => &f.native.boundary,
                        _ => &f.store.finish,
                    };
                    boundary.config.lock().unwrap().envelope = Some(wrong.clone());
                    if empty == 1 {
                        boundary.config.lock().unwrap().fault =
                            Some(Fault::Domain(SubscriptionError::SignInRequired, false));
                    }
                    if empty == 2 && stage == 0 {
                        *f.store.record.lock().unwrap() = None;
                    }
                    refused(f.tokens(false, None).await);
                    assert_eq!(boundary.calls(), 1);
                }
            }
        }
    }
}

#[tokio::test]
async fn matching_envelopes_do_not_certify_nested_records_or_caches() {
    let (root, process) = policy();
    let first = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let (_, foreign) = policy();
    let policies = [
        process,
        first.policy().clone(),
        second.policy().clone(),
        foreign,
    ];
    for (receiving, policy) in policies.iter().enumerate() {
        for (source, wrong) in policies.iter().enumerate() {
            if receiving == source {
                continue;
            }
            let f = Fixture::with_policy(root.clone(), policy.clone(), 1);
            let record = f.store.record().unwrap();
            *f.store.record.lock().unwrap() = Some(own(wrong, || (*record).clone()));
            refused(f.tokens(false, None).await);
            assert_eq!(f.store.claim.calls(), 0);
            *f.store.record.lock().unwrap() = Some(record);
            *f.native.renewed.lock().unwrap() = own_cache(wrong, "account", 7200);
            refused(f.tokens(false, None).await);
            assert!(f.store.record().unwrap().cache.is_some());
            assert_eq!(f.native.calls(), 1);
        }
    }
}

#[tokio::test]
async fn dynamic_error_custody_is_checked_for_immediate_and_polled_failures() {
    let (root, process) = policy();
    let first = process.admit_attachment().unwrap();
    let second = process.admit_attachment().unwrap();
    let (_, foreign) = policy();
    let policies = [
        process,
        first.policy().clone(),
        second.policy().clone(),
        foreign,
    ];
    for (receiving, policy) in policies.iter().enumerate() {
        for (source, error_policy) in policies.iter().enumerate() {
            for immediate in [true, false] {
                let f = Fixture::with_policy(root.clone(), policy.clone(), 3600);
                let diagnostic = own(error_policy, || {
                    layer3_adapter::AdapterError::Unavailable("synthetic diagnostic".into())
                });
                let error = AdmittedError::Adapter(diagnostic.clone());
                f.store.get.config.lock().unwrap().fault = Some(if immediate {
                    Fault::Immediate(error)
                } else {
                    Fault::Source(error, false)
                });
                let result = f.tokens(false, None).await;
                if receiving != source {
                    refused(result);
                } else {
                    let AdmittedError::Adapter(retained) = result.unwrap_err() else {
                        panic!("lost diagnostic category")
                    };
                    assert!(retained.ptr_eq(&diagnostic));
                }
                assert_eq!(f.store.get.calls(), usize::from(!immediate));
            }
        }
    }
}

#[test]
fn engine_owner_must_match_the_retained_owned_identity() {
    let f = Fixture::new(3600);
    let port = Admitted {
        manager: &f.manager,
        owner: &f.owner,
        policy: f.policy.clone(),
    };
    assert!(matches!(
        port.get(&SubjectId::new("another")).now_or_never().unwrap(),
        Err(Error::Source(_))
    ));
    assert_eq!(f.store.get.calls(), 0);
}
