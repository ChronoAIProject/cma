//! Telling the three service refusals apart.
//!
//! "You never granted this", "we cannot ask right now" and "this deployment
//! has no catalogue" are three answers, and a caller who cannot distinguish
//! them does the wrong thing for two of them: they retry the one that will
//! never succeed, and give up on the one that would have worked a moment
//! later. Only one of the three is an error at all.

use crate::environment::ServiceId;
use crate::environment_support::{profile_spec, service_catalog_service, FakeServiceCatalog};
use crate::error::CoreError;
use crate::session::SessionSpec;
use crate::test_support::{service_with_environments, user, FakeAdapter, FakeStore};
use std::sync::Arc;

/// Start a session from a profile naming these services.
async fn start_with(
    environments: Arc<crate::environment_service::EnvironmentProfileService>,
    services: &[&str],
) -> Result<crate::session::Session, CoreError> {
    let owner = user("subject-a");
    let mut spec = profile_spec("scoped");
    for id in services {
        spec.services.push(ServiceId::new(*id));
    }
    let profile = environments.create(&owner, spec).await.expect("creates");
    let service = service_with_environments(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
        environments,
    );
    service
        .create_session(
            &owner,
            SessionSpec {
                environment_profile: Some(profile.id),
                ..crate::test_support::spec()
            },
        )
        .await
}

#[tokio::test]
async fn an_ungranted_service_is_refused_by_name() {
    // Not transient, and the answer is to grant it in NyxID. A session started
    // against a service its owner has not given would run perfectly and refuse
    // every call the agent made on their behalf.
    let environments =
        service_catalog_service(Some(Arc::new(FakeServiceCatalog::granting(["api-lark"]))));

    let refusal = start_with(environments, &["api-github"])
        .await
        .expect_err("a service the owner has not granted is refused");

    assert_eq!(refusal.code(), "service_not_granted");
    assert!(
        refusal.to_string().contains("api-github"),
        "a refusal that does not name the service is one nobody can act on: {refusal}"
    );
}

#[tokio::test]
async fn a_granted_service_starts_a_session() {
    let environments =
        service_catalog_service(Some(Arc::new(FakeServiceCatalog::granting(["api-github"]))));
    let session = start_with(environments, &["api-github"])
        .await
        .expect("a granted service starts");
    let granted = session.granted_services.expect("recorded");
    assert!(granted.contains(&ServiceId::new("api-github")));
}

#[tokio::test]
async fn an_unreachable_catalogue_does_not_block_a_stored_selection() {
    // What a session may reach is recorded on the profile; the catalogue
    // exists to help choose, not to authorise. Blocking here would take the
    // service down with NyxID, and would do so for selections settled days
    // ago.
    let environments = service_catalog_service(Some(Arc::new(FakeServiceCatalog::unreachable())));

    let session = start_with(environments, &["api-github"])
        .await
        .expect("an unreachable catalogue refuses nothing");

    let granted = session.granted_services.expect("recorded");
    assert!(granted.contains(&ServiceId::new("api-github")));
}

#[tokio::test]
async fn a_deployment_with_no_catalogue_refuses_nothing() {
    // Not an error at all: profiles still store selections and sessions still
    // honour them, there is simply nothing to choose from. A deployment that
    // cannot list services has not thereby withheld any.
    let environments = service_catalog_service(None);

    let session = start_with(environments, &["api-github"])
        .await
        .expect("no catalogue means nothing was withheld");

    let granted = session.granted_services.expect("recorded");
    assert!(granted.contains(&ServiceId::new("api-github")));
}

#[tokio::test]
async fn the_baseline_is_never_refused() {
    // Mandated services are what a session IS on this platform rather than
    // something its owner opted into, so a catalogue that does not list one
    // has not denied it.
    let environments = service_catalog_service(Some(Arc::new(FakeServiceCatalog::granting([]))));

    let session = start_with(environments, &[])
        .await
        .expect("the deployment's own floor is not the owner's to grant");

    let granted = session.granted_services.expect("recorded");
    assert!(granted.contains(&ServiceId::new("nyxid")));
}

#[tokio::test]
async fn a_grant_revoked_after_the_profile_was_written_is_refused_at_provision() {
    // The reason the check is at provision and not at write. The profile was
    // valid when it was saved; by the time a session is started from it, it is
    // not — and refusing at write would have said nothing about this.
    let owner = user("subject-a");
    let environments =
        service_catalog_service(Some(Arc::new(FakeServiceCatalog::granting(["api-github"]))));
    let mut spec = profile_spec("scoped");
    spec.services.push(ServiceId::new("api-github"));
    let profile = environments.create(&owner, spec).await.expect("creates");

    // The owner takes the grant back in NyxID. The profile is untouched.
    let revoked = service_catalog_service(Some(Arc::new(FakeServiceCatalog::granting([]))));
    let stored = environments
        .get(&owner, &profile.id)
        .await
        .expect("still stored");
    let mut respec = profile_spec(&stored.name);
    respec.services = stored.services.clone();
    let reprofile = revoked.create(&owner, respec).await.expect("creates");

    let service = service_with_environments(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
        revoked,
    );
    let refusal = service
        .create_session(
            &owner,
            SessionSpec {
                environment_profile: Some(reprofile.id),
                ..crate::test_support::spec()
            },
        )
        .await
        .expect_err("a revoked grant is refused when a session is started");

    assert_eq!(refusal.code(), "service_not_granted");
    assert!(refusal.to_string().contains("api-github"));
}

#[tokio::test]
async fn the_three_answers_are_distinguishable_at_the_catalogue() {
    use crate::environment::ServiceAvailability;
    let owner = user("subject-a");

    // No catalogue: not an error, and it grants nothing it could refuse with.
    let none = service_catalog_service(None);
    let answer = none
        .service_availability(&owner, None)
        .await
        .expect("not an error");
    assert!(matches!(answer, ServiceAvailability::NoCatalogue));
    assert!(answer.grants().is_none());

    // Granted nothing: a real answer, and one that CAN refuse.
    let empty = service_catalog_service(Some(Arc::new(FakeServiceCatalog::granting([]))));
    let answer = empty
        .service_availability(&owner, None)
        .await
        .expect("not an error");
    assert_eq!(answer.grants().expect("a catalogue answered").len(), 0);

    // Unreachable: the only one of the three that is an error, and it is not
    // the one a caller would blame the sandbox provider for.
    let down = service_catalog_service(Some(Arc::new(FakeServiceCatalog::unreachable())));
    let refusal = down
        .service_availability(&owner, None)
        .await
        .expect_err("could not be asked");
    assert_eq!(refusal.code(), "service_catalogue_unavailable");
    assert_ne!(
        refusal.code(),
        CoreError::RuntimeUnavailable("x".into()).code()
    );
}

#[tokio::test]
async fn no_refusal_carries_credential_material() {
    // The message names what is wrong and never quotes a value, which is the
    // one part of any of this that may be a credential.
    let refusal = CoreError::ServiceNotGranted("api-github".into()).to_string();
    assert!(!refusal.contains("token"));
    assert!(!refusal.contains("Bearer"));

    let down = CoreError::ServiceCatalogUnavailable("connection refused".into()).to_string();
    assert!(!down.contains("token"));
    assert!(!down.contains("Bearer"));
}
