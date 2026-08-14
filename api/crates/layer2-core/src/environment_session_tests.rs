//! Starting a session from an environment profile.
//!
//! The boundary these cover is the one that matters in production: what an
//! owner stored has to be what the adapter is asked to provision with, and a
//! selection the caller may not use has to fail before a sandbox exists.

use super::*;

/// Wait until the adapter has been asked to provision something.
///
/// Bringing a runtime up is detached from the request that asked for it, so
/// creating a session returns before any of this has happened. Reading the
/// adapter immediately would be reading it before the work started.
async fn await_provisioned(adapter: &Arc<FakeAdapter>) -> layer3_adapter::RuntimeSpec {
    for _ in 0..400 {
        if let Some(spec) = adapter.last_spec.lock().await.clone() {
            return spec;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("nothing was ever provisioned");
}

#[tokio::test]
async fn a_selected_profile_reaches_the_runtime_that_backs_the_session() {
    // End to end through the core: what an owner stored is what the adapter is
    // asked to provision with.
    let profiles = Arc::new(FakeProfileStore::default());
    let environment_service = environments(Arc::clone(&profiles));
    let owner = user("subject-a");
    let mut spec = profile_spec("build tools");
    spec.variables
        .push(EnvironmentVariable::new("BUILD_MODE", "release"));
    spec.packages.push(SoftwarePackage::new("ripgrep"));
    spec.agent.skills.push(SkillId::new("chosen-skill"));
    let profile = environment_service
        .create(&owner, spec)
        .await
        .expect("creates");

    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        environment_service,
    );
    sessions
        .create_session(
            &owner,
            crate::session::SessionSpec {
                environment_profile: Some(profile.id.clone()),
                ..crate::test_support::spec()
            },
        )
        .await
        .expect("session created");

    let provisioned = await_provisioned(&adapter).await;
    assert_eq!(
        provisioned
            .environment
            .variables
            .get("BUILD_MODE")
            .map(String::as_str),
        Some("release")
    );
    assert!(provisioned
        .environment
        .packages
        .contains(&"ripgrep".to_string()));
    assert!(provisioned
        .environment
        .skills
        .contains(&"chosen-skill".to_string()));
    // And the platform's own entries alongside them, unasked for.
    assert!(provisioned
        .environment
        .packages
        .contains(&"platform-cli".to_string()));
    assert!(provisioned
        .environment
        .skills
        .contains(&"mandatory-one".to_string()));
}

#[tokio::test]
async fn a_session_started_without_a_profile_still_carries_the_baseline() {
    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        environments(Arc::new(FakeProfileStore::default())),
    );
    sessions
        .create_session(&user("subject-a"), crate::test_support::spec())
        .await
        .expect("session created");

    let provisioned = await_provisioned(&adapter).await;
    assert!(provisioned.environment.variables.is_empty());
    assert!(provisioned
        .environment
        .packages
        .contains(&"platform-cli".to_string()));
    assert_eq!(provisioned.environment.skills.len(), 2);
}

#[tokio::test]
async fn a_session_is_refused_rather_than_started_with_someone_elses_profile() {
    // Refused before anything is provisioned: a sandbox that exists with the
    // wrong environment is worse than a request that failed.
    let profiles = Arc::new(FakeProfileStore::default());
    let environment_service = environments(Arc::clone(&profiles));
    let theirs = environment_service
        .create(&user("subject-a"), profile_spec("theirs"))
        .await
        .expect("creates");

    let adapter = Arc::new(FakeAdapter::default());
    let sessions = service_with_environments(
        Arc::new(FakeStore::default()),
        Arc::clone(&adapter),
        environment_service,
    );
    let err = sessions
        .create_session(
            &user("subject-b"),
            crate::session::SessionSpec {
                environment_profile: Some(theirs.id.clone()),
                ..crate::test_support::spec()
            },
        )
        .await
        .expect_err("must be refused");
    assert_eq!(err, CoreError::EnvironmentProfileNotFound);
    assert!(
        adapter.last_spec.lock().await.is_none(),
        "nothing may be provisioned for a refused request"
    );
}
