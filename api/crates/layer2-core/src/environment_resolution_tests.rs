//! What a session is actually started with, and what a caller may choose from.
//!
//! Split from the ownership tests because the two answer different questions.
//! Those ask who may touch a profile; these ask what comes out of one — which
//! is where the baseline rule lives, and it is the rule most worth being able
//! to read in one sitting: some entries are part of what a session *is* on this
//! platform, so a client that omits them still gets them and a client that
//! deletes them gets them back.

use super::*;
use crate::authz::Authorization;
use crate::environment::{CatalogError, EnvironmentVariable, SkillId, SoftwarePackage};
use crate::ids::SubjectId;
use crate::test_support::{
    catalog_service, environments, profile_spec, service_with_environments, user, FakeAdapter,
    FakeProfileStore, FakeSkillCatalog, FakeStore,
};

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.to_string()),
        Authorization::from_capabilities([Capability::ManageOwnEnvironmentProfiles]),
    )
}

#[tokio::test]
async fn a_client_that_omits_the_baseline_still_gets_it() {
    // The guarantee, end to end through the service: what is stored, what is
    // read back, and what a session is given all carry the mandatory entries.
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");

    let created = service
        .create(&owner, profile_spec("bare"))
        .await
        .expect("creates");
    assert!(created.packages.iter().any(|p| p.name == "platform-cli"));
    assert!(created
        .agent
        .skills
        .contains(&SkillId::new("mandatory-one")));

    let resolved = service
        .resolve(&owner, Some(&created.id))
        .await
        .expect("resolves");
    assert!(resolved.packages.contains(&"platform-cli".to_string()));
    assert!(resolved.skills.contains(&"mandatory-one".to_string()));
    assert!(resolved.skills.contains(&"mandatory-two".to_string()));
}

#[tokio::test]
async fn a_client_that_tries_to_remove_the_baseline_gets_it_back() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let created = service
        .create(&owner, profile_spec("bare"))
        .await
        .expect("creates");

    // The removal a client would actually attempt: PUT the resource back
    // without the entries it does not want.
    let stripped = profile_spec("bare");
    let updated = service
        .update(&owner, &created.id, stripped)
        .await
        .expect("updates");
    assert!(updated.packages.iter().any(|p| p.name == "platform-cli"));
    assert_eq!(updated.agent.skills.len(), 2);
}

#[tokio::test]
async fn a_profile_stored_before_the_baseline_existed_still_resolves_with_it() {
    // Written by a service that mandated nothing, read by one that does. The
    // second must not hand out a session missing the platform's own entries.
    let store = Arc::new(FakeProfileStore::default());
    let bare = EnvironmentProfileService::new(
        Arc::clone(&store) as Arc<dyn EnvironmentProfileStore>,
        Baseline::default(),
        None,
        None,
        Default::default(),
    );
    let owner = caller("subject-a");
    let created = bare
        .create(&owner, profile_spec("legacy"))
        .await
        .expect("creates");
    assert!(created.packages.is_empty());

    let current = environments(store);
    let resolved = current
        .resolve(&owner, Some(&created.id))
        .await
        .expect("resolves");
    assert!(resolved.packages.contains(&"platform-cli".to_string()));
}

#[tokio::test]
async fn selecting_no_profile_still_yields_the_baseline() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let resolved = service
        .resolve(&caller("subject-a"), None)
        .await
        .expect("resolves");
    assert!(resolved.variables.is_empty());
    assert!(resolved.packages.contains(&"platform-cli".to_string()));
    assert_eq!(resolved.skills.len(), 2);
}

#[tokio::test]
async fn a_caller_cannot_start_a_session_from_someone_elses_profile() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let mine = service
        .create(&caller("subject-a"), profile_spec("mine"))
        .await
        .expect("creates");

    let err = service
        .resolve(&caller("subject-b"), Some(&mine.id))
        .await
        .expect_err("must be refused");
    assert_eq!(err, CoreError::EnvironmentProfileNotFound);
}

#[tokio::test]
async fn resolving_carries_the_variables_a_session_needs() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let mut spec = profile_spec("mine");
    spec.variables
        .push(EnvironmentVariable::new("BUILD_MODE", "release"));
    spec.packages.push(SoftwarePackage {
        name: "ripgrep".into(),
        version: Some("14".into()),
    });
    let created = service.create(&owner, spec).await.expect("creates");

    let resolved = service
        .resolve(&owner, Some(&created.id))
        .await
        .expect("resolves");
    assert_eq!(
        resolved.variables.get("BUILD_MODE").map(String::as_str),
        Some("release")
    );
    assert!(resolved.packages.contains(&"ripgrep@14".to_string()));
}

#[tokio::test]
async fn an_invalid_profile_is_rejected_rather_than_stored() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let mut spec = profile_spec("bad");
    spec.variables
        .push(EnvironmentVariable::new("NOT A NAME", "x"));

    let err = service.create(&owner, spec).await.expect_err("rejected");
    assert_eq!(err.code(), "invalid_request");
    assert!(
        store.profiles.lock().await.is_empty(),
        "a rejected profile must not be written"
    );
}

#[tokio::test]
async fn an_unreachable_store_is_reported_as_unavailable_not_as_missing() {
    // Telling a caller their profile does not exist when the database is down
    // invites them to recreate something that is still there.
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let created = service
        .create(&owner, profile_spec("mine"))
        .await
        .expect("creates");
    store.fail_everything().await;

    for err in [
        service.get(&owner, &created.id).await.expect_err("fails"),
        service.list(&owner).await.expect_err("fails"),
        service
            .create(&owner, profile_spec("another"))
            .await
            .expect_err("fails"),
        service
            .resolve(&owner, Some(&created.id))
            .await
            .expect_err("fails"),
    ] {
        assert!(
            matches!(err, CoreError::RuntimeUnavailable(_)),
            "got {err:?}"
        );
    }
}

#[tokio::test]
async fn the_catalogue_lists_what_the_caller_may_choose() {
    let catalog = Arc::new(FakeSkillCatalog::with_skills(["one", "two"]));
    let service = catalog_service(Arc::clone(&catalog));
    let owner = caller("subject-a");

    let skills = service
        .available_skills(&owner, Some("delegated-credential"))
        .await
        .expect("lists");
    assert_eq!(skills.len(), 2);
    // The caller's own credential is what makes the answer theirs rather than
    // this service's, so it must actually reach the catalogue.
    assert_eq!(
        catalog.last_credential.lock().await.as_deref(),
        Some("delegated-credential")
    );
}

#[tokio::test]
async fn a_deployment_with_no_catalogue_offers_an_empty_list_rather_than_failing() {
    // No catalogue is a degraded picker, not a broken feature: profiles still
    // store and resolve skills chosen by other means.
    let service = environments(Arc::new(FakeProfileStore::default()));
    let skills = service
        .available_skills(&caller("subject-a"), None)
        .await
        .expect("succeeds");
    assert!(skills.is_empty());
}

#[tokio::test]
async fn an_unreachable_catalogue_is_reported() {
    let catalog = Arc::new(FakeSkillCatalog::broken(CatalogError::Unavailable(
        "connection refused".into(),
    )));
    let service = catalog_service(catalog);
    let err = service
        .available_skills(&caller("subject-a"), None)
        .await
        .expect_err("must be reported");
    assert!(
        matches!(err, CoreError::RuntimeUnavailable(_)),
        "got {err:?}"
    );
}

#[tokio::test]
async fn browsing_the_catalogue_needs_the_same_capability_as_the_profiles() {
    let service = catalog_service(Arc::new(FakeSkillCatalog::with_skills(["one"])));
    let unarmed = Caller::new(
        SubjectId("subject-a".into()),
        Authorization::from_capabilities([Capability::UseOwnSessions]),
    );
    assert_eq!(
        service
            .available_skills(&unarmed, None)
            .await
            .expect_err("refused"),
        CoreError::Forbidden
    );
}

/// How a session is actually started from a profile.
///
/// A separate file because it exercises the session service as well as this
/// one, and because keeping both here would push the module past the project's
/// file-size limit.
#[path = "environment_session_tests.rs"]
mod session;

/// Telling the three service refusals apart.
///
/// A separate file for the same reason, and because what it covers is one
/// question rather than a corner of another: only one of the three answers a
/// service catalogue can give is an error.
#[path = "service_refusal_tests.rs"]
mod service_refusals;

#[tokio::test]
async fn a_profile_that_named_no_kernel_runs_what_this_deployment_runs() {
    // Layer 1 leaves it unset because it does not know which kernels exist
    // here. If nothing settled it, the profile would store an empty selector
    // and every session started from it would fail to find a runtime.
    let store = Arc::new(FakeProfileStore::default());
    let service = EnvironmentProfileService::new(
        Arc::clone(&store) as Arc<dyn EnvironmentProfileStore>,
        Baseline::default(),
        None,
        None,
        crate::environment::AgentConfiguration {
            kernel: crate::ids::KernelId::new("fake"),
            ..Default::default()
        },
    );
    let owner = caller("subject-a");

    let mut spec = profile_spec("unspecified");
    spec.agent.kernel = crate::ids::KernelId::default();
    let created = service.create(&owner, spec).await.expect("created");

    assert_eq!(
        created.agent.kernel,
        crate::ids::KernelId::new("fake"),
        "a stored profile must say what it will actually run"
    );
    assert_eq!(
        service
            .agent_for(&owner, Some(&created.id))
            .await
            .expect("resolves")
            .kernel,
        crate::ids::KernelId::new("fake")
    );
}
