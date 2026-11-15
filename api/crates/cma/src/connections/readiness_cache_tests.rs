use super::*;

fn rows(services: &[ServiceId], state: ServiceRequirementState) -> Vec<ServiceRequirement> {
    services
        .iter()
        .map(|service| ServiceRequirement {
            service: service.clone(),
            state,
        })
        .collect()
}
fn seed(cache: &ReadinessCache, owner: &SubjectId, services: &[ServiceId]) {
    let ticket = cache.begin(owner, services);
    cache.finish(
        ticket,
        Some(&rows(services, ServiceRequirementState::Satisfied)),
    );
}

#[tokio::test(start_paused = true)]
async fn readiness_cache_requires_complete_owner_scoped_evidence_within_thirty_seconds() {
    let cache = ReadinessCache::default();
    let owner = SubjectId::new("one");
    let services = vec![ServiceId::new("a"), ServiceId::new("b")];
    seed(&cache, &owner, &services);
    assert!(cache.get(&SubjectId::new("two"), &services).is_none());
    assert!(cache.get(&owner, &[ServiceId::new("c")]).is_none());
    assert!(cache.get(&owner, &[]).is_none());
    assert_eq!(cache.get(&owner, &services).unwrap().valid_for_ms, 30_000);
    tokio::time::advance(Duration::from_secs(29)).await;
    assert_eq!(cache.get(&owner, &services).unwrap().valid_for_ms, 1000);
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(cache.get(&owner, &services).is_none());
}

#[tokio::test]
async fn readiness_cache_revocation_refusal_and_unknown_remove_positive_evidence() {
    for state in [
        ServiceRequirementState::Missing,
        ServiceRequirementState::Inactive,
        ServiceRequirementState::Unknown,
    ] {
        let cache = ReadinessCache::default();
        let owner = SubjectId::new("owner");
        let services = vec![ServiceId::new("connection")];
        seed(&cache, &owner, &services);
        let ticket = cache.begin(&owner, &services);
        cache.finish(ticket, Some(&rows(&services, state)));
        assert!(cache.get(&owner, &services).is_none());
        seed(&cache, &owner, &services);
        let ticket = cache.begin(&owner, &services);
        cache.finish(ticket, None);
        assert!(cache.get(&owner, &services).is_none());
    }
}

#[tokio::test]
async fn readiness_cache_late_success_cannot_restore_newer_revocation_or_replaced_entry() {
    let cache = ReadinessCache::default();
    let owner = SubjectId::new("owner");
    let services = vec![ServiceId::new("connection")];
    let old = cache.begin(&owner, &services);
    let newer = cache.begin(&owner, &services);
    cache.finish(newer, None);
    cache.finish(
        old,
        Some(&rows(&services, ServiceRequirementState::Satisfied)),
    );
    assert!(cache.get(&owner, &services).is_none());
    let old = cache.begin(&owner, &services);
    cache.invalidate(&owner);
    let newer = cache.begin(&owner, &services);
    cache.finish(newer, None);
    cache.finish(
        old,
        Some(&rows(&services, ServiceRequirementState::Satisfied)),
    );
    assert!(cache.get(&owner, &services).is_none());
}

#[tokio::test]
async fn readiness_cache_is_bounded_and_large_scopes_never_use_partial_evidence() {
    let cache = ReadinessCache::default();
    let owner = SubjectId::new("owner");
    for n in 0..=CAPACITY {
        seed(&cache, &owner, &[ServiceId::new(format!("service-{n:05}"))]);
    }
    assert_eq!(cache.state.lock().unwrap().entries.len(), CAPACITY);
    assert!(cache
        .get(&owner, &[ServiceId::new("service-00000")])
        .is_none());
    let large = (0..=MAX_SERVICES)
        .map(|n| ServiceId::new(format!("new-{n}")))
        .collect::<Vec<_>>();
    seed(&cache, &owner, &large);
    assert!(cache.get(&owner, &large).is_none());
    assert_eq!(cache.state.lock().unwrap().entries.len(), CAPACITY);
}
