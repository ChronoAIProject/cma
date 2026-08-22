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

/// The grant a session was provisioned with, recorded on the session itself.
///
/// A profile can be edited after a session started from it, and the session
/// keeps the scope it was provisioned with. Reading the profile to learn a
/// session's reach gives the wrong answer precisely when it matters — which is
/// after somebody has changed the bundle a live session was started from.
mod granted_services {
    use super::*;
    use crate::environment::{Baseline, ServiceId};
    use crate::environment_support::{fake_agent, profile_spec};
    use crate::environment_values::SkillId;
    use crate::environment_values::SoftwarePackage;
    use crate::session::SessionSpec;
    use crate::store::SessionStore;
    use crate::test_support::{service_with_environments, user, FakeAdapter, FakeStore};

    /// A baseline with one mandated service, so the floor is visible.
    fn environments_with_mandated(
        profiles: Arc<FakeProfileStore>,
    ) -> Arc<EnvironmentProfileService> {
        EnvironmentProfileService::new(
            profiles,
            Baseline::new(
                [SoftwarePackage::new("platform-cli")],
                [SkillId::new("mandatory-one")],
                [ServiceId::new("nyxid")],
            ),
            None,
            None,
            fake_agent(),
            crate::test_support::unconfigured_settings(),
        )
    }

    #[tokio::test]
    async fn a_session_records_what_it_was_granted() {
        let profiles = Arc::new(FakeProfileStore::default());
        let environment_service = environments_with_mandated(Arc::clone(&profiles));
        let owner = user("subject-a");
        let mut spec = profile_spec("github work");
        spec.services.push(ServiceId::new("github"));
        let profile = environment_service
            .create(&owner, spec)
            .await
            .expect("creates");

        let store = Arc::new(FakeStore::default());
        let service = service_with_environments(
            Arc::clone(&store),
            Arc::new(FakeAdapter::default()),
            environment_service,
        );
        let session = service
            .create_session(
                &owner,
                SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("creates");

        let granted = session.granted_services.expect("the grant is recorded");
        assert!(granted.contains(&ServiceId::new("github")));
        // The deployment's floor travels with it: a session that lost its
        // model endpoint because a bundle was tidied up is not a session.
        assert!(granted.contains(&ServiceId::new("nyxid")));
    }

    #[tokio::test]
    async fn editing_the_profile_afterwards_does_not_change_a_running_session() {
        // The whole point. A profile is a reusable record and a session's reach
        // is a fact about that session, decided once.
        let profiles = Arc::new(FakeProfileStore::default());
        let environment_service = environments_with_mandated(Arc::clone(&profiles));
        let owner = user("subject-a");
        let mut spec = profile_spec("github work");
        spec.services.push(ServiceId::new("github"));
        let profile = environment_service
            .create(&owner, spec)
            .await
            .expect("creates");

        let store = Arc::new(FakeStore::default());
        let service = service_with_environments(
            Arc::clone(&store),
            Arc::new(FakeAdapter::default()),
            Arc::clone(&environment_service),
        );
        let session = service
            .create_session(
                &owner,
                SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("creates");

        // Widen the profile after the session started from it.
        let mut widened = profile_spec("github work");
        widened.services.push(ServiceId::new("github"));
        widened.services.push(ServiceId::new("lark"));
        environment_service
            .update(&owner, &profile.id, widened)
            .await
            .expect("updates");

        let reread = service
            .get_session(&owner, &session.id)
            .await
            .expect("reads");
        let granted = reread.granted_services.expect("the grant is recorded");
        assert!(
            !granted.contains(&ServiceId::new("lark")),
            "a running session must not acquire reach its owner gave a profile afterwards"
        );

        // And the credential minted for it is scoped the same way.
        let scoped = service.session_services(&session.id).await;
        assert!(!scoped.contains(&ServiceId::new("lark")));
        assert!(scoped.contains(&ServiceId::new("github")));
    }

    #[tokio::test]
    async fn a_session_recorded_before_the_grant_existed_falls_back_to_its_profile() {
        // `None` is not an empty grant. One says the answer was never written
        // down and has to be resolved; the other says this session may reach
        // nothing, and must not be widened by a lookup.
        let profiles = Arc::new(FakeProfileStore::default());
        let environment_service = environments_with_mandated(Arc::clone(&profiles));
        let owner = user("subject-a");
        let mut spec = profile_spec("github work");
        spec.services.push(ServiceId::new("github"));
        let profile = environment_service
            .create(&owner, spec)
            .await
            .expect("creates");

        let store = Arc::new(FakeStore::default());
        let service = service_with_environments(
            Arc::clone(&store),
            Arc::new(FakeAdapter::default()),
            environment_service,
        );
        let session = service
            .create_session(
                &owner,
                SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("creates");

        // Rewrite the record the way an older control plane would have left it.
        let mut record = store
            .get(&session.id)
            .await
            .expect("reads")
            .expect("exists");
        record.session.granted_services = None;
        store.upsert(&record).await.expect("writes");

        let scoped = service.session_services(&session.id).await;
        assert!(
            scoped.contains(&ServiceId::new("github")),
            "a session already running must not lose its reach because this field did not exist"
        );
    }

    #[tokio::test]
    async fn the_grant_survives_a_rotation() {
        let profiles = Arc::new(FakeProfileStore::default());
        let environment_service = environments_with_mandated(Arc::clone(&profiles));
        let owner = user("subject-a");
        let mut spec = profile_spec("github work");
        spec.services.push(ServiceId::new("github"));
        let profile = environment_service
            .create(&owner, spec)
            .await
            .expect("creates");

        let store = Arc::new(FakeStore::default());
        let service = service_with_environments(
            Arc::clone(&store),
            Arc::new(FakeAdapter::default()),
            environment_service,
        );
        let session = service
            .create_session(
                &owner,
                SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("creates");

        // A rotation replaces the runtime and keeps the session. The grant is
        // part of the session, so it is one of the things that has to survive.
        let mut record = store
            .get(&session.id)
            .await
            .expect("reads")
            .expect("exists");
        record.session.rotations += 1;
        record.session.runtime_version = Some("next".into());
        store.upsert(&record).await.expect("writes");

        let scoped = service.session_services(&session.id).await;
        assert!(scoped.contains(&ServiceId::new("github")));
    }

    #[tokio::test]
    async fn the_runtime_is_told_what_the_session_may_reach() {
        // Recording the grant is half of it. The half that changes what
        // happens is the agent knowing: one that does not know it may act on a
        // service reasons about why it cannot, and answers confidently and
        // wrongly.
        let profiles = Arc::new(FakeProfileStore::default());
        let environment_service = environments_with_mandated(Arc::clone(&profiles));
        let owner = user("subject-a");
        let mut spec = profile_spec("github work");
        spec.services.push(ServiceId::new("github"));
        let profile = environment_service
            .create(&owner, spec)
            .await
            .expect("creates");

        let adapter = Arc::new(FakeAdapter::default());
        let service = service_with_environments(
            Arc::new(FakeStore::default()),
            Arc::clone(&adapter),
            environment_service,
        );
        service
            .create_session(
                &owner,
                SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("creates");

        let provisioned = await_provisioned(&adapter).await;
        assert!(provisioned.reaches.contains(&"github".to_string()));
        assert!(
            provisioned.reaches.contains(&"nyxid".to_string()),
            "the deployment's floor travels with the grant"
        );
    }

    #[tokio::test]
    async fn a_session_that_reaches_nothing_hands_the_runtime_an_empty_list() {
        // Not the absence of the mechanism. A runtime told "nothing" can say
        // so; one told nothing at all cannot tell that apart from a control
        // plane that never mentioned it.
        let profiles = Arc::new(FakeProfileStore::default());
        let environment_service = EnvironmentProfileService::new(
            profiles,
            Baseline::new([], [], []),
            None,
            None,
            fake_agent(),
            crate::test_support::unconfigured_settings(),
        );
        let owner = user("subject-a");
        let profile = environment_service
            .create(&owner, profile_spec("no services"))
            .await
            .expect("creates");

        let adapter = Arc::new(FakeAdapter::default());
        let service = service_with_environments(
            Arc::new(FakeStore::default()),
            Arc::clone(&adapter),
            environment_service,
        );
        service
            .create_session(
                &owner,
                SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("creates");

        assert!(await_provisioned(&adapter).await.reaches.is_empty());
    }

    #[tokio::test]
    async fn what_the_runtime_is_told_carries_no_credential() {
        // Names are a fact about a session's configuration; the token minted
        // against them is not, and travels a different path that never enters
        // the sandbox.
        let profiles = Arc::new(FakeProfileStore::default());
        let environment_service = environments_with_mandated(Arc::clone(&profiles));
        let owner = user("subject-a");
        let mut spec = profile_spec("github work");
        spec.services.push(ServiceId::new("github"));
        let profile = environment_service
            .create(&owner, spec)
            .await
            .expect("creates");

        let adapter = Arc::new(FakeAdapter::default());
        let service = service_with_environments(
            Arc::new(FakeStore::default()),
            Arc::clone(&adapter),
            environment_service,
        );
        service
            .create_session(
                &owner,
                SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    caller_credential: Some("a-real-credential-value".into()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("creates");

        let provisioned = await_provisioned(&adapter).await;
        for name in &provisioned.reaches {
            assert!(
                !name.contains("a-real-credential-value"),
                "a credential value reached the list of names"
            );
        }
    }
}

/// The size frozen onto a session and used by every runtime it receives.
mod session_sizes {
    use super::*;
    use crate::store::SessionStore;
    use crate::test_support::{await_settled, service_with_environments_on_version};
    use crate::{Session, SessionService, SessionSize};

    fn size(cpu_millis: u32, memory_mib: u32) -> SessionSize {
        SessionSize::new(cpu_millis, memory_mib).expect("a non-zero size is usable")
    }

    fn profile_with_size(name: &str, requested: Option<SessionSize>) -> EnvironmentProfileSpec {
        let mut spec = profile_spec(name);
        spec.agent.size = requested;
        spec
    }

    async fn save_profile(
        service: &Arc<EnvironmentProfileService>,
        owner: &Caller,
        name: &str,
        requested: Option<SessionSize>,
    ) -> EnvironmentProfile {
        service
            .create(owner, profile_with_size(name, requested))
            .await
            .expect("profile creates")
    }

    async fn create_from(
        service: &Arc<SessionService>,
        owner: &Caller,
        profile: &EnvironmentProfile,
    ) -> Session {
        service
            .create_session(
                owner,
                crate::session::SessionSpec {
                    environment_profile: Some(profile.id.clone()),
                    ..crate::test_support::spec()
                },
            )
            .await
            .expect("session creates")
    }

    fn assert_capacity(
        provisioned: &layer3_adapter::RuntimeSpec,
        cpu_millis: u32,
        memory_mib: u32,
    ) {
        let capacity = provisioned
            .capacity
            .expect("the requested capacity crosses");
        assert_eq!(capacity.cpu_millis.get(), cpu_millis);
        assert_eq!(capacity.memory_mib.get(), memory_mib);
    }

    #[tokio::test]
    async fn a_profile_size_reaches_the_runtime_that_backs_the_session() {
        let environment_service = environments(Arc::new(FakeProfileStore::default()));
        let owner = user("subject-a");
        let profile = save_profile(
            &environment_service,
            &owner,
            "large build",
            Some(size(1_500, 3_072)),
        )
        .await;
        let adapter = Arc::new(FakeAdapter::default());
        let sessions = service_with_environments(
            Arc::new(FakeStore::default()),
            Arc::clone(&adapter),
            environment_service,
        );

        create_from(&sessions, &owner, &profile).await;

        assert_capacity(&await_provisioned(&adapter).await, 1_500, 3_072);
    }

    #[tokio::test]
    async fn a_profile_that_names_no_size_provisions_at_the_deployments_own() {
        let environment_service = environments(Arc::new(FakeProfileStore::default()));
        let owner = user("subject-a");
        let profile = save_profile(&environment_service, &owner, "ordinary build", None).await;
        let adapter = Arc::new(FakeAdapter::default());
        let sessions = service_with_environments(
            Arc::new(FakeStore::default()),
            Arc::clone(&adapter),
            environment_service,
        );

        create_from(&sessions, &owner, &profile).await;

        assert_eq!(await_provisioned(&adapter).await.capacity, None);
    }

    #[tokio::test]
    async fn a_session_recorded_before_sizing_provisions_at_the_deployments_own() {
        let environment_service = environments(Arc::new(FakeProfileStore::default()));
        let owner = user("subject-a");
        let profile = save_profile(
            &environment_service,
            &owner,
            "old session",
            Some(size(1_500, 3_072)),
        )
        .await;
        let store = Arc::new(FakeStore::default());
        let adapter = Arc::new(FakeAdapter::default());
        let sessions = service_with_environments(
            Arc::clone(&store),
            Arc::clone(&adapter),
            Arc::clone(&environment_service),
        );
        let session = create_from(&sessions, &owner, &profile).await;
        await_settled(&sessions, &owner, &session.id).await;
        sessions
            .stop_session(&owner, &session.id)
            .await
            .expect("session stops");

        let mut record = store
            .get(&session.id)
            .await
            .expect("record reads")
            .expect("record exists");
        let mut stored = serde_json::to_value(&record.session).expect("session serializes");
        let removed = stored["options"]
            .as_object_mut()
            .expect("options are an object")
            .remove("size");
        assert!(removed.is_some(), "the fixture must first carry a size");
        record.session = serde_json::from_value(stored).expect("an older session reads");
        assert_eq!(record.session.options.size, None);
        store.upsert(&record).await.expect("older record writes");

        *adapter.last_spec.lock().await = None;
        drop(sessions);
        let restarted = service_with_environments(
            Arc::clone(&store),
            Arc::clone(&adapter),
            environment_service,
        );
        restarted
            .revive_session(&owner, &session.id)
            .await
            .expect("older session revives");

        assert_eq!(await_provisioned(&adapter).await.capacity, None);
    }

    #[tokio::test]
    async fn a_session_keeps_its_size_when_its_profile_changes() {
        let environment_service = environments(Arc::new(FakeProfileStore::default()));
        let owner = user("subject-a");
        let profile = save_profile(
            &environment_service,
            &owner,
            "resized later",
            Some(size(1_500, 3_072)),
        )
        .await;
        let adapter = Arc::new(FakeAdapter::default());
        let sessions = service_with_environments(
            Arc::new(FakeStore::default()),
            Arc::clone(&adapter),
            Arc::clone(&environment_service),
        );
        let session = create_from(&sessions, &owner, &profile).await;
        await_settled(&sessions, &owner, &session.id).await;
        sessions
            .stop_session(&owner, &session.id)
            .await
            .expect("session stops");
        environment_service
            .update(
                &owner,
                &profile.id,
                profile_with_size("resized later", Some(size(3_000, 6_144))),
            )
            .await
            .expect("profile resizes");

        *adapter.last_spec.lock().await = None;
        sessions
            .revive_session(&owner, &session.id)
            .await
            .expect("session revives");

        assert_capacity(&await_provisioned(&adapter).await, 1_500, 3_072);
    }

    #[tokio::test]
    async fn a_rotation_carries_a_sessions_size() {
        let environment_service = environments(Arc::new(FakeProfileStore::default()));
        let owner = user("subject-a");
        let profile = save_profile(
            &environment_service,
            &owner,
            "rotating session",
            Some(size(1_500, 3_072)),
        )
        .await;
        let store = Arc::new(FakeStore::default());
        let adapter = Arc::new(FakeAdapter::default());
        let old = service_with_environments_on_version(
            Arc::clone(&store),
            Arc::clone(&adapter),
            Arc::clone(&environment_service),
            "runtime:1",
        );
        let session = create_from(&old, &owner, &profile).await;
        await_settled(&old, &owner, &session.id).await;
        *adapter.last_spec.lock().await = None;
        drop(old);

        let new = service_with_environments_on_version(
            store,
            Arc::clone(&adapter),
            environment_service,
            "runtime:2",
        );
        let reconciled = new.reconcile().await;

        assert_eq!(reconciled.rotated_sessions, 1);
        assert_capacity(&await_provisioned(&adapter).await, 1_500, 3_072);
    }

    #[tokio::test]
    async fn a_lost_runtime_is_replaced_at_the_same_size() {
        let environment_service = environments(Arc::new(FakeProfileStore::default()));
        let owner = user("subject-a");
        let profile = save_profile(
            &environment_service,
            &owner,
            "recovering session",
            Some(size(1_500, 3_072)),
        )
        .await;
        let store = Arc::new(FakeStore::default());
        let adapter = Arc::new(FakeAdapter::default());
        let first = service_with_environments(
            Arc::clone(&store),
            Arc::clone(&adapter),
            Arc::clone(&environment_service),
        );
        let session = create_from(&first, &owner, &profile).await;
        await_settled(&first, &owner, &session.id).await;
        let lost = store
            .get(&session.id)
            .await
            .expect("record reads")
            .expect("record exists")
            .runtime_id
            .expect("runtime exists");
        drop(first);
        adapter.lose(&lost).await;
        *adapter.last_spec.lock().await = None;

        let restarted = service_with_environments(store, Arc::clone(&adapter), environment_service);
        let reconciled = restarted.reconcile().await;

        assert_eq!(reconciled.reprovisioned_sessions, 1);
        assert_capacity(&await_provisioned(&adapter).await, 1_500, 3_072);
    }
}
