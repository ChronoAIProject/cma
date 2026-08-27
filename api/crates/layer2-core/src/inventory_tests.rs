//! Tests for what is running and whose it is.
//!
//! The case that matters most is the one that produces no runtimes for two
//! entirely different reasons, so most of these are about telling "nothing is
//! running" apart from "nobody could be asked".

use std::sync::Arc;
use std::time::Duration;

use crate::authz::{Authorization, Caller, Capability};
use crate::commit_fault_support::CommitFaultStore;
use crate::coordination::{SessionBoundary, SessionLeaseRequest, SessionOperation};
use crate::error::CoreError;
use crate::event::Event;
use crate::ids::{SessionId, SubjectId};
use crate::inventory::{ProviderView, RuntimeAuthorityComparison};
use crate::store::{SessionRecord, SessionStore, SkillPackState};
use crate::test_support::{
    await_settled, service, service_over, spec, user, FakeAdapter, FakeStore,
};
use crate::RuntimeCleanupReason;

fn operator() -> Caller {
    Caller::new(
        SubjectId("operator".into()),
        Authorization::from_capabilities([Capability::ReadRuntimeInventory]),
    )
}

fn no_commits(_record: &SessionRecord, _events: &[Event]) -> bool {
    false
}

#[tokio::test]
async fn a_running_session_appears_with_its_owner() {
    // The join this exists for: the provider knows what is running, the record
    // knows whose it is, and neither answer is useful on its own.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&owner, spec()).await.expect("created");
    await_settled(&svc, &owner, &created.id).await;

    let inventory = svc
        .runtime_inventory(&operator())
        .await
        .expect("an operator may read the inventory");

    assert_eq!(inventory.running(), Some(1));
    assert!(inventory.fully_observed());
    let ProviderView::Observed(runtimes) = &inventory.providers[0].view else {
        panic!("the provider answered and must be reported as having answered");
    };
    assert_eq!(runtimes[0].session.as_ref(), Some(&created.id));
    assert_eq!(
        runtimes[0].owner.as_ref().map(|o| o.as_str()),
        Some("alice")
    );
    assert!(!runtimes[0].is_unaccounted());
    assert_eq!(
        runtimes[0].durable_authority.as_ref().unwrap().runtime_id,
        Some(runtimes[0].runtime.clone())
    );
    assert_eq!(
        runtimes[0].authority_comparison,
        RuntimeAuthorityComparison::Matches,
        "provider and durable generations agree for coordinated provisioning"
    );
}

#[tokio::test]
async fn provider_attribution_point_reads_only_referenced_sessions() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), no_commits));
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("alice");
    let svc = service_over(store.clone(), Arc::clone(&adapter));
    let created = svc.create_session(&owner, spec()).await.expect("created");
    await_settled(&svc, &owner, &created.id).await;

    let template = inner
        .get(&created.id)
        .await
        .expect("readable")
        .expect("the provider-linked session exists");
    for index in 0..200 {
        let mut unrelated = template.clone();
        unrelated.session.id = SessionId(format!("sesn_z{index:04}"));
        unrelated.session.owner = SubjectId("unrelated".into());
        inner
            .seed(&unrelated)
            .await
            .expect("seed unrelated session");
    }

    let reads_before = store.get_calls();
    let inventory = svc
        .runtime_inventory(&operator())
        .await
        .expect("the referenced durable row is readable");
    assert_eq!(
        store.get_calls() - reads_before,
        1,
        "unrelated durable history must not cause point reads"
    );
    let ProviderView::Observed(runtimes) = &inventory.providers[0].view else {
        panic!("the provider answered and must be reported as having answered");
    };
    let linked = runtimes
        .iter()
        .find(|runtime| runtime.session.as_ref() == Some(&created.id))
        .expect("the provider-linked runtime is present");
    assert_eq!(
        linked.owner.as_ref().map(SubjectId::as_str),
        Some("alice"),
        "the unique provider-referenced session is still attributed"
    );
}

#[tokio::test]
async fn provider_attribution_rejects_a_projected_exact_read() {
    let inner = Arc::new(FakeStore::default());
    let store = Arc::new(CommitFaultStore::new(Arc::clone(&inner), no_commits));
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("projected-inventory-owner");
    let svc = service_over(store.clone(), Arc::clone(&adapter));
    let created = svc.create_session(&owner, spec()).await.expect("created");
    await_settled(&svc, &owner, &created.id).await;
    let mut exact = inner.get(&created.id).await.unwrap().unwrap();
    exact.skill_packs = SkillPackState::Staged(Vec::new());
    inner.seed(&exact).await.expect("seed exact staged custody");
    store.project_staged_gets(true);

    assert!(matches!(
        svc.runtime_inventory(&operator()).await,
        Err(CoreError::RuntimeUnavailable(_))
    ));
}

#[tokio::test]
async fn a_provider_runtime_is_labelled_with_its_exact_cleanup_debt() {
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let owner = user("alice");
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let created = svc.create_session(&owner, spec()).await.expect("created");
    await_settled(&svc, &owner, &created.id).await;
    let record = store.get(&created.id).await.unwrap().unwrap();
    let legacy_runtime = record.runtime_id.clone().unwrap();
    let authority = store.runtime_authority(&created.id).await.unwrap().unwrap();
    let request = SessionLeaseRequest::new(
        created.id.clone(),
        "inventory-test",
        SessionBoundary {
            status: record.session.status,
            authority,
        },
        SessionOperation::Handover,
        Duration::from_secs(30),
    );
    let mut lease = store.acquire_session_lease(&request).await.unwrap();
    let reservation = store
        .reserve_runtime_generation(&lease, "inventory-candidate")
        .await
        .unwrap();
    adapter
        .running
        .lock()
        .await
        .insert("runtime-current".into(), Some(created.id.0.clone()));
    adapter
        .generations
        .lock()
        .await
        .insert("runtime-current".into(), reservation.generation);
    let candidate = store
        .register_runtime_candidate(&lease, &reservation, "runtime-current")
        .await
        .unwrap();
    store
        .switch_runtime_authority(&mut lease, &candidate)
        .await
        .unwrap();

    let inventory = svc.runtime_inventory(&operator()).await.unwrap();
    let ProviderView::Observed(runtimes) = &inventory.providers[0].view else {
        panic!("the fake provider is observable")
    };
    let retired = runtimes
        .iter()
        .find(|runtime| runtime.runtime == legacy_runtime)
        .unwrap();
    assert_eq!(
        retired.cleanup_reason,
        Some(RuntimeCleanupReason::RetiredAuthority)
    );
    let current = runtimes
        .iter()
        .find(|runtime| runtime.runtime == candidate.runtime_id)
        .unwrap();
    assert_eq!(current.cleanup_reason, None);
    assert_eq!(
        current.authority_comparison,
        RuntimeAuthorityComparison::Matches
    );
}

#[tokio::test]
async fn a_provider_that_cannot_be_asked_is_not_reported_as_running_nothing() {
    // The whole point of the type. An operator shown an empty namespace when
    // the truth is that nobody could be asked will conclude the fleet is idle.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter {
        refuse_list: true,
        ..FakeAdapter::default()
    });
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let inventory = svc.runtime_inventory(&operator()).await.expect("readable");

    assert!(!inventory.fully_observed());
    assert_eq!(
        inventory.running(),
        None,
        "a total that omits a provider nobody could ask is not a total"
    );
    let ProviderView::Unobservable(reason) = &inventory.providers[0].view else {
        panic!("a provider that refused must be reported as unobservable");
    };
    assert!(!reason.is_empty(), "the refusal must say why");
}

#[tokio::test]
async fn a_provider_running_nothing_is_reported_as_running_nothing() {
    // The other half of the distinction: an answer of zero is an answer.
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));

    let inventory = svc.runtime_inventory(&operator()).await.expect("readable");

    assert_eq!(inventory.running(), Some(0));
    assert!(inventory.fully_observed());
    assert!(matches!(
        inventory.providers[0].view,
        ProviderView::Observed(_)
    ));
}

#[tokio::test]
async fn a_runtime_no_session_accounts_for_is_visible_as_such() {
    // What is left behind when a process died between provisioning a sandbox
    // and recording it. It is running, it belongs to nobody, and an operator
    // must see it even though census data alone cannot authorise reclamation.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    adapter
        .running
        .lock()
        .await
        .insert("runtime-stray".into(), Some("sesn_forgotten".into()));
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));

    let inventory = svc.runtime_inventory(&operator()).await.expect("readable");

    let stray: Vec<_> = inventory.unaccounted().collect();
    assert_eq!(stray.len(), 1);
    assert_eq!(stray[0].runtime, "runtime-stray");
    assert!(stray[0].owner.is_none());
    assert!(stray[0].status.is_none());
    assert_eq!(
        stray[0].authority_comparison,
        RuntimeAuthorityComparison::Unaccounted
    );
}

#[tokio::test]
async fn reading_the_inventory_needs_the_capability_for_it() {
    let store = Arc::new(FakeStore::default());
    let svc = service(Arc::clone(&store), Arc::new(FakeAdapter::default()));
    assert_eq!(
        svc.runtime_inventory(&user("alice")).await.err(),
        Some(CoreError::Forbidden),
        "what everybody else is running is not an ordinary caller's business"
    );
}

#[tokio::test]
async fn an_unreadable_record_fails_rather_than_producing_anonymous_runtimes() {
    // Without the record nothing can be attributed to anybody, and a list of
    // sandboxes belonging to nobody would read as a fleet full of orphans.
    let store = Arc::new(FakeStore::default());
    let adapter = Arc::new(FakeAdapter::default());
    let svc = service(Arc::clone(&store), Arc::clone(&adapter));
    let owner = user("alice");
    let created = svc.create_session(&owner, spec()).await.expect("created");
    await_settled(&svc, &owner, &created.id).await;
    store.fail_everything().await;

    let err = svc
        .runtime_inventory(&operator())
        .await
        .expect_err("an unreadable record must not be reported as an inventory");
    assert!(
        matches!(err, CoreError::RuntimeUnavailable(_)),
        "got {err:?}"
    );
}
