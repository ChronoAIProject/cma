//! Tests for environment profile ownership, enforcement, and resolution.
//!
//! Kept in a sibling file so `environment_service.rs` stays within the
//! project's file-size limit while the behaviour remains covered.

use super::*;
use crate::authz::Authorization;
use crate::environment::{EnvironmentVariable, ServiceId, SkillId, SoftwarePackage};
use crate::ids::{KernelId, SubjectId};
use crate::test_support::{
    environments, fake_agent, profile_spec, unconfigured_settings, FakeProfileStore,
    ReplacementInterference,
};

fn environments_with_baseline(
    store: Arc<FakeProfileStore>,
    baseline: Baseline,
) -> Arc<EnvironmentProfileService> {
    EnvironmentProfileService::new(
        store,
        baseline,
        None,
        None,
        fake_agent(),
        unconfigured_settings(),
    )
}

fn caller(subject: &str) -> Caller {
    Caller::new(
        SubjectId(subject.to_string()),
        Authorization::from_capabilities([Capability::ManageOwnEnvironmentProfiles]),
    )
}

fn admin() -> Caller {
    Caller::new(
        SubjectId("admin".into()),
        Authorization::from_capabilities([
            Capability::ReadAllSessions,
            Capability::ReadRuntimeInventory,
            Capability::ReadEventLog,
        ]),
    )
}

fn first_page() -> PageRequest {
    PageRequest::new(200, None)
}

#[tokio::test]
async fn a_created_profile_is_readable_by_its_owner() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");

    let created = service
        .create(&owner, profile_spec("build tools"))
        .await
        .expect("creates");
    assert_eq!(created.owner, owner.subject);
    assert_eq!(created.name, "build tools");

    let read = service.get(&owner, &created.id).await.expect("reads");
    assert_eq!(read.id, created.id);
    assert_eq!(
        service
            .list(&owner, &first_page())
            .await
            .expect("lists")
            .items
            .len(),
        1
    );
}

#[tokio::test]
async fn a_profile_name_is_stored_trimmed() {
    // A name with stray whitespace is the same name; storing it verbatim would
    // produce two entries in a picker that look identical.
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let mut spec = profile_spec("  padded  ");
    spec.variables.push(EnvironmentVariable::new("A", "1"));

    let created = service.create(&owner, spec).await.expect("creates");
    assert_eq!(created.name, "padded");
}

#[tokio::test]
async fn one_subject_cannot_read_anothers_profile() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let mine = service
        .create(&caller("subject-a"), profile_spec("mine"))
        .await
        .expect("creates");

    let err = service
        .get(&caller("subject-b"), &mine.id)
        .await
        .expect_err("must be refused");
    assert_eq!(err, CoreError::EnvironmentProfileNotFound);
}

#[tokio::test]
async fn one_subject_cannot_modify_or_remove_anothers_profile() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let mine = service
        .create(&caller("subject-a"), profile_spec("mine"))
        .await
        .expect("creates");

    let intruder = caller("subject-b");
    assert_eq!(
        service
            .update(&intruder, &mine.id, profile_spec("theirs"))
            .await
            .expect_err("must be refused"),
        CoreError::EnvironmentProfileNotFound
    );
    assert_eq!(
        service
            .delete(&intruder, &mine.id)
            .await
            .expect_err("must be refused"),
        CoreError::EnvironmentProfileNotFound
    );
    // Refused means untouched, not merely unreported.
    assert!(store.profiles.lock().await.contains_key(mine.id.as_str()));
    assert_eq!(
        service
            .get(&caller("subject-a"), &mine.id)
            .await
            .expect("still there")
            .name,
        "mine"
    );
}

#[tokio::test]
async fn listing_shows_only_the_callers_own_profiles() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    service
        .create(&caller("subject-a"), profile_spec("a-one"))
        .await
        .expect("creates");
    service
        .create(&caller("subject-a"), profile_spec("a-two"))
        .await
        .expect("creates");
    service
        .create(&caller("subject-b"), profile_spec("b-one"))
        .await
        .expect("creates");

    let mine = service
        .list(&caller("subject-a"), &first_page())
        .await
        .expect("lists");
    assert_eq!(mine.items.len(), 2);
    assert!(mine.items.iter().all(|p| p.owner.as_str() == "subject-a"));
}

#[tokio::test]
async fn a_privileged_reader_still_cannot_read_a_profile() {
    // Reading every session is the monitoring surface. A profile holds values
    // its owner may treat as secret, so no read capability reaches one.
    let service = environments(Arc::new(FakeProfileStore::default()));
    let mine = service
        .create(&caller("subject-a"), profile_spec("mine"))
        .await
        .expect("creates");

    assert_eq!(
        service
            .get(&admin(), &mine.id)
            .await
            .expect_err("must be refused"),
        CoreError::Forbidden,
        "an admin holds no capability over environment profiles at all"
    );
}

#[tokio::test]
async fn a_caller_without_the_capability_is_refused_every_operation() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let created = service
        .create(&owner, profile_spec("mine"))
        .await
        .expect("creates");

    let unarmed = Caller::new(
        SubjectId("subject-a".into()),
        Authorization::from_capabilities([Capability::CreateSession]),
    );
    assert_eq!(
        service
            .create(&unarmed, profile_spec("x"))
            .await
            .expect_err("refused"),
        CoreError::Forbidden
    );
    assert_eq!(
        service
            .get(&unarmed, &created.id)
            .await
            .expect_err("refused"),
        CoreError::Forbidden
    );
    assert_eq!(
        service
            .list(&unarmed, &first_page())
            .await
            .expect_err("refused"),
        CoreError::Forbidden
    );
    assert_eq!(
        service
            .update(&unarmed, &created.id, profile_spec("x"))
            .await
            .expect_err("refused"),
        CoreError::Forbidden
    );
    assert_eq!(
        service
            .delete(&unarmed, &created.id)
            .await
            .expect_err("refused"),
        CoreError::Forbidden
    );
}

#[tokio::test]
async fn an_update_replaces_contents_but_keeps_identity_and_creation_time() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let mut spec = profile_spec("first");
    spec.variables.push(EnvironmentVariable::new("ONE", "1"));
    let created = service.create(&owner, spec).await.expect("creates");

    let mut replacement = profile_spec("second");
    replacement
        .variables
        .push(EnvironmentVariable::new("TWO", "2"));
    let updated = service
        .update(&owner, &created.id, replacement)
        .await
        .expect("updates");

    assert_eq!(updated.id, created.id);
    assert_eq!(updated.owner, created.owner);
    assert_eq!(updated.created_at, created.created_at);
    assert_eq!(updated.name, "second");
    assert_eq!(updated.variables.len(), 1);
    assert_eq!(updated.variables[0].key, "TWO");
}

#[tokio::test]
async fn caller_limits_allow_exact_echoes_beside_the_current_baseline() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments_with_baseline(
        Arc::clone(&store),
        Baseline::new(
            [
                SoftwarePackage::new("mandatory-package"),
                SoftwarePackage::new("mandatory-package"),
            ],
            [
                SkillId::new("mandatory-skill"),
                SkillId::new("mandatory-skill"),
            ],
            [
                ServiceId::new("mandatory-service"),
                ServiceId::new("mandatory-service"),
            ],
        ),
    );
    let owner = caller("subject-a");
    let mut boundary = profile_spec("boundary");
    boundary.packages = (0..100)
        .map(|i| SoftwarePackage::new(format!("caller-package-{i}")))
        .collect();
    boundary.agent.skills = (0..100)
        .map(|i| SkillId::new(format!("caller-skill-{i}")))
        .collect();
    boundary.services = (0..100)
        .map(|i| ServiceId::new(format!("caller-service-{i}")))
        .collect();

    let created = service
        .create(&owner, boundary)
        .await
        .expect("100 caller entries plus each baseline entry create");
    assert_eq!(created.packages.len(), 101);
    assert_eq!(created.agent.skills.len(), 101);
    assert_eq!(created.services.len(), 101);
    assert_eq!(
        created
            .packages
            .iter()
            .filter(|package| package.name == "mandatory-package")
            .count(),
        1
    );
    assert_eq!(
        created
            .agent
            .skills
            .iter()
            .filter(|skill| skill.as_str() == "mandatory-skill")
            .count(),
        1
    );
    assert_eq!(
        created
            .services
            .iter()
            .filter(|service| service.as_str() == "mandatory-service")
            .count(),
        1
    );

    let echoed = service
        .update(&owner, &created.id, created.to_spec())
        .await
        .expect("the normalized response is a valid exact echo");
    assert_eq!(echoed.to_spec(), created.to_spec());
    assert_eq!(echoed.updated_at, created.updated_at);
    assert_eq!(
        store
            .content_replacements
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "an exact boundary echo performs no content write"
    );

    let mut too_many_packages = profile_spec("too many packages");
    too_many_packages.packages = (0..101)
        .map(|i| SoftwarePackage::new(format!("caller-package-{i}")))
        .collect();
    assert!(service.create(&owner, too_many_packages).await.is_err());

    let mut too_many_skills = profile_spec("too many skills");
    too_many_skills.agent.skills = (0..101)
        .map(|i| SkillId::new(format!("caller-skill-{i}")))
        .collect();
    assert!(service.create(&owner, too_many_skills).await.is_err());

    let mut too_many_services = profile_spec("too many services");
    too_many_services.services = (0..101)
        .map(|i| ServiceId::new(format!("caller-service-{i}")))
        .collect();
    assert!(service.create(&owner, too_many_services).await.is_err());
}

#[tokio::test]
async fn a_legacy_profile_remains_echoable_after_the_baseline_grows_past_raw_limits() {
    let store = Arc::new(FakeProfileStore::default());
    let old_service = environments_with_baseline(Arc::clone(&store), Baseline::default());
    let owner = caller("subject-a");
    let mut historical = profile_spec("historical boundary");
    historical.packages = (0..100)
        .map(|i| SoftwarePackage::new(format!("caller-package-{i}")))
        .collect();
    historical.agent.skills = (0..100)
        .map(|i| SkillId::new(format!("caller-skill-{i}")))
        .collect();
    historical.services = (0..100)
        .map(|i| ServiceId::new(format!("caller-service-{i}")))
        .collect();
    let stored = old_service
        .create(&owner, historical)
        .await
        .expect("the old deployment stores its boundary");
    let raw_before = store
        .profiles
        .lock()
        .await
        .get(stored.id.as_str())
        .expect("the historical row exists")
        .clone();

    let current_service = environments_with_baseline(
        Arc::clone(&store),
        Baseline::new(
            [SoftwarePackage::new("new-mandatory-package")],
            [SkillId::new("new-mandatory-skill")],
            [ServiceId::new("new-mandatory-service")],
        ),
    );
    let projected = current_service
        .get(&owner, &stored.id)
        .await
        .expect("the legacy boundary projects through the new baseline");
    assert_eq!(projected.packages.len(), 101);
    assert_eq!(projected.agent.skills.len(), 101);
    assert_eq!(projected.services.len(), 101);

    let echoed = current_service
        .update(&owner, &stored.id, projected.to_spec())
        .await
        .expect("the post-growth projection remains echoable");
    assert_eq!(echoed.to_spec(), projected.to_spec());
    assert_eq!(echoed.updated_at, raw_before.updated_at);
    assert_eq!(
        store
            .content_replacements
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let raw_after = store.profiles.lock().await;
    let raw_after = raw_after.get(stored.id.as_str()).expect("row remains");
    assert_eq!(raw_after.to_spec(), raw_before.to_spec());
    assert_eq!(raw_after.updated_at, raw_before.updated_at);
}

#[tokio::test]
async fn only_an_exact_no_op_bypasses_bounds_after_mandatory_identities_are_retired() {
    let store = Arc::new(FakeProfileStore::default());
    let old_service = environments_with_baseline(
        Arc::clone(&store),
        Baseline::new(
            [SoftwarePackage::new("old-mandatory-package")],
            [SkillId::new("old-mandatory-skill")],
            [ServiceId::new("old-mandatory-service")],
        ),
    );
    let owner = caller("subject-a");
    let mut historical = profile_spec("retired baseline boundary");
    historical.packages = (0..100)
        .map(|i| SoftwarePackage::new(format!("caller-package-{i}")))
        .collect();
    historical.agent.skills = (0..100)
        .map(|i| SkillId::new(format!("caller-skill-{i}")))
        .collect();
    historical.services = (0..100)
        .map(|i| ServiceId::new(format!("caller-service-{i}")))
        .collect();
    let stored = old_service
        .create(&owner, historical)
        .await
        .expect("the old deployment stores its boundary and mandatory entries");
    let raw_before = store
        .profiles
        .lock()
        .await
        .get(stored.id.as_str())
        .expect("the historical row exists")
        .clone();

    let current_service = environments_with_baseline(
        Arc::clone(&store),
        Baseline::new(
            [SoftwarePackage::new("new-mandatory-package")],
            [SkillId::new("new-mandatory-skill")],
            [ServiceId::new("new-mandatory-service")],
        ),
    );
    let projected = current_service
        .get(&owner, &stored.id)
        .await
        .expect("the renamed baseline is projected without rewriting storage");
    assert_eq!(projected.packages.len(), 102);
    assert_eq!(projected.agent.skills.len(), 102);
    assert_eq!(projected.services.len(), 102);

    let echoed = current_service
        .update(&owner, &stored.id, projected.to_spec())
        .await
        .expect("the exact normalized projection is a no-op despite retired identities");
    assert_eq!(echoed.to_spec(), projected.to_spec());
    assert_eq!(echoed.updated_at, raw_before.updated_at);
    assert_eq!(
        store
            .content_replacements
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the exact projection does not call replace_content"
    );
    let raw_after_echo = store
        .profiles
        .lock()
        .await
        .get(stored.id.as_str())
        .expect("the historical row remains")
        .clone();
    assert_eq!(
        serde_json::to_value(&raw_after_echo).expect("stored profile serializes"),
        serde_json::to_value(&raw_before).expect("stored profile serializes"),
        "the no-op does not normalize the stored row or advance updated_at"
    );

    let mut changed_packages = projected.to_spec();
    changed_packages.name = "changed packages".into();
    changed_packages.agent.skills = vec![SkillId::new("new-mandatory-skill")];
    changed_packages.services = vec![ServiceId::new("new-mandatory-service")];

    let mut changed_skills = projected.to_spec();
    changed_skills.name = "changed skills".into();
    changed_skills.packages = vec![SoftwarePackage::new("new-mandatory-package")];
    changed_skills.services = vec![ServiceId::new("new-mandatory-service")];

    let mut changed_services = projected.to_spec();
    changed_services.name = "changed services".into();
    changed_services.packages = vec![SoftwarePackage::new("new-mandatory-package")];
    changed_services.agent.skills = vec![SkillId::new("new-mandatory-skill")];

    for (collection, changed) in [
        ("packages", changed_packages),
        ("skills", changed_skills),
        ("services", changed_services),
    ] {
        let error = current_service
            .update(&owner, &stored.id, changed)
            .await
            .expect_err("a changed write must validate retired identities as caller-controlled");
        let CoreError::InvalidRequest(detail) = error else {
            panic!("expected an invalid {collection} request, got {error:?}");
        };
        assert!(
            detail.contains(collection),
            "the independent {collection} refusal names its bound: {detail}"
        );
    }

    assert_eq!(
        store
            .content_replacements
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no refused changed write reaches replace_content"
    );
    let raw_after_refusals = store
        .profiles
        .lock()
        .await
        .get(stored.id.as_str())
        .expect("the historical row remains")
        .clone();
    assert_eq!(
        serde_json::to_value(raw_after_refusals).expect("stored profile serializes"),
        serde_json::to_value(raw_before).expect("stored profile serializes")
    );
}

#[tokio::test]
async fn replacement_preserves_a_preference_set_after_the_service_read() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let profile = service
        .create(&owner, profile_spec("before"))
        .await
        .expect("creates");
    assert!(!profile.preferred);
    store
        .interfere_with_next_replacement(ReplacementInterference::Preferred(true))
        .await;

    let replaced = service
        .update(&owner, &profile.id, profile_spec("after"))
        .await
        .expect("replaces");

    assert!(replaced.preferred);
    assert!(
        store
            .profiles
            .lock()
            .await
            .get(profile.id.as_str())
            .expect("row remains")
            .preferred
    );
}

#[tokio::test]
async fn replacement_preserves_a_preference_cleared_after_the_service_read() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let profile = service
        .create(&owner, profile_spec("before"))
        .await
        .expect("creates");
    service
        .prefer(&owner, Some(&profile.id))
        .await
        .expect("sets the stale value");
    store
        .interfere_with_next_replacement(ReplacementInterference::Preferred(false))
        .await;

    let replaced = service
        .update(&owner, &profile.id, profile_spec("after"))
        .await
        .expect("replaces");

    assert!(!replaced.preferred);
    assert!(
        !store
            .profiles
            .lock()
            .await
            .get(profile.id.as_str())
            .expect("row remains")
            .preferred
    );
}

#[tokio::test]
async fn replacement_refuses_a_profile_deleted_after_the_service_read() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let profile = service
        .create(&owner, profile_spec("before"))
        .await
        .expect("creates");
    store
        .interfere_with_next_replacement(ReplacementInterference::Delete)
        .await;

    let error = service
        .update(&owner, &profile.id, profile_spec("after"))
        .await
        .expect_err("a deleted row cannot be replaced");

    assert_eq!(error, CoreError::EnvironmentProfileNotFound);
    assert!(!store
        .profiles
        .lock()
        .await
        .contains_key(profile.id.as_str()));
    assert_eq!(
        store
            .content_replacements
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "replacement never resurrects the deleted identifier"
    );
}

#[tokio::test]
async fn a_legacy_read_echo_projects_current_defaults_without_rewriting() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let created = service
        .create(&owner, profile_spec("unchanged"))
        .await
        .expect("creates");
    let mut historical = created.clone();
    historical.updated_at = "2025-01-01T00:00:00Z".into();
    historical.packages.clear();
    historical.agent.kernel = KernelId::default();
    historical.agent.skills.clear();
    store
        .upsert(&historical)
        .await
        .expect("seeds old timestamp");

    let read = service
        .get(&owner, &historical.id)
        .await
        .expect("legacy profile reads");
    assert_eq!(read.agent.kernel, KernelId::new("fake"));
    assert_eq!(read.packages, service.baseline().packages());
    assert_eq!(read.agent.skills, service.baseline().skills());
    let listed = service
        .list(&owner, &first_page())
        .await
        .expect("legacy profile lists");
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].to_spec(), read.to_spec());

    let echoed = service
        .update(&owner, &historical.id, read.to_spec())
        .await
        .expect("an echo succeeds");

    assert_eq!(echoed.updated_at, historical.updated_at);
    assert_eq!(echoed.to_spec(), read.to_spec());
    assert_eq!(
        store
            .profiles
            .lock()
            .await
            .get(historical.id.as_str())
            .expect("legacy record remains")
            .to_spec(),
        historical.to_spec(),
        "read normalization and an exact echo do not migrate raw storage"
    );
}

#[tokio::test]
async fn a_removed_profile_is_gone() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let created = service
        .create(&owner, profile_spec("mine"))
        .await
        .expect("creates");

    service.delete(&owner, &created.id).await.expect("removes");
    assert_eq!(
        service.get(&owner, &created.id).await.expect_err("gone"),
        CoreError::EnvironmentProfileNotFound
    );
    assert!(service
        .list(&owner, &first_page())
        .await
        .expect("lists")
        .items
        .is_empty());
}

#[tokio::test]
async fn removing_something_that_never_existed_is_reported_not_ignored() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let err = service
        .delete(
            &caller("subject-a"),
            &EnvironmentProfileId("envp_nope".into()),
        )
        .await
        .expect_err("must be reported");
    assert_eq!(err, CoreError::EnvironmentProfileNotFound);
}

/* Client-side profile preselection. At most one profile per owner carries it. */

#[tokio::test]
async fn a_new_profile_is_not_preselected_for_its_owner() {
    // Authoring a profile does not also choose the client's initial selection.
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");

    let created = service
        .create(&owner, profile_spec("build tools"))
        .await
        .expect("creates");

    assert!(!created.preferred);
}

#[tokio::test]
async fn choosing_a_preselection_clears_the_one_before_it() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let first = service
        .create(&owner, profile_spec("first"))
        .await
        .expect("creates");
    let second = service
        .create(&owner, profile_spec("second"))
        .await
        .expect("creates");

    service.prefer(&owner, Some(&first.id)).await.expect("sets");
    let after = service
        .prefer(&owner, Some(&second.id))
        .await
        .expect("moves");

    let held: Vec<_> = after
        .iter()
        .filter(|p| p.preferred)
        .map(|p| &p.id)
        .collect();
    assert_eq!(held, vec![&second.id], "exactly one, and it is the new one");
    assert_eq!(after.len(), 2, "only the two visibly changed rows return");
}

#[tokio::test]
async fn preference_changes_stay_bounded_beyond_a_complete_store_page() {
    use std::sync::atomic::Ordering;

    use crate::store::MAX_PAGE_SIZE;

    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let mut ids = Vec::new();
    for sequence in 0..=MAX_PAGE_SIZE {
        let profile = service
            .create(&owner, profile_spec(&format!("profile {sequence}")))
            .await
            .expect("creates");
        ids.push(profile.id);
    }

    service
        .prefer(&owner, ids.first())
        .await
        .expect("sets the first preference");
    let changed = service
        .prefer(&owner, ids.last())
        .await
        .expect("moves the preference across the page boundary");

    assert_eq!(changed.len(), 2);
    assert!(!changed[0].preferred);
    assert!(changed[1].preferred);
    assert_eq!(changed[1].id, *ids.last().expect("a final id"));
    assert_eq!(store.list_calls.load(Ordering::SeqCst), 0);
    assert_eq!(store.preference_writes.load(Ordering::SeqCst), 2);
    let stored = store.profiles.lock().await;
    let held: Vec<_> = stored
        .values()
        .filter(|profile| profile.preferred)
        .map(|profile| profile.id.clone())
        .collect();
    assert_eq!(held, vec![ids.last().expect("a final id").clone()]);
}

#[tokio::test]
async fn an_explicit_null_clears_the_client_preselection() {
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let profile = service
        .create(&owner, profile_spec("first"))
        .await
        .expect("creates");
    service
        .prefer(&owner, Some(&profile.id))
        .await
        .expect("sets");

    let after = service.prefer(&owner, None).await.expect("clears");

    assert!(after.iter().all(|p| !p.preferred));
}

#[tokio::test]
async fn a_preselection_survives_an_edit_of_the_profile_holding_it() {
    // Editing a profile is independent of the client's initial selection.
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");
    let profile = service
        .create(&owner, profile_spec("first"))
        .await
        .expect("creates");
    service
        .prefer(&owner, Some(&profile.id))
        .await
        .expect("sets");

    let edited = service
        .update(&owner, &profile.id, profile_spec("first, renamed"))
        .await
        .expect("updates");

    assert!(edited.preferred);
}

#[tokio::test]
async fn a_profile_somebody_else_owns_cannot_be_preselected() {
    use std::sync::atomic::Ordering;

    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let stranger = caller("subject-b");
    let theirs = service
        .create(&owner, profile_spec("theirs"))
        .await
        .expect("creates");
    service
        .prefer(&owner, Some(&theirs.id))
        .await
        .expect("the owner may select it");

    let refused = service.prefer(&stranger, Some(&theirs.id)).await;

    assert!(
        matches!(refused, Err(CoreError::EnvironmentProfileNotFound)),
        "not yours and does not exist collapse to one answer, so a stranger \
         cannot probe for what exists"
    );
    assert_eq!(store.preference_writes.load(Ordering::SeqCst), 1);
    assert!(
        store
            .profiles
            .lock()
            .await
            .get(&theirs.id.0)
            .expect("the profile remains")
            .preferred,
        "validation happens before the existing preference is cleared"
    );
}

#[tokio::test]
async fn two_profiles_claiming_preselection_are_settled_on_read() {
    /*
     * There is no transaction across the store port, so a `prefer` that
     * half-succeeds can leave two claims. The read settles it rather than
     * leaving it to be discovered: the most recently updated wins.
     */
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let older = service
        .create(&owner, profile_spec("older"))
        .await
        .expect("creates");
    let newer = service
        .create(&owner, profile_spec("newer"))
        .await
        .expect("creates");

    // Both written directly, as a torn write would leave them.
    for (profile, updated) in [
        (&older, "2026-01-01T00:00:00Z"),
        (&newer, "2026-06-01T00:00:00Z"),
    ] {
        let mut claim = profile.clone();
        claim.preferred = true;
        claim.updated_at = updated.to_string();
        store.upsert(&claim).await.expect("writes");
    }

    let listed = service.list(&owner, &first_page()).await.expect("lists");

    let held: Vec<_> = listed
        .items
        .iter()
        .filter(|p| p.preferred)
        .map(|p| &p.id)
        .collect();
    assert_eq!(held, vec![&newer.id], "one claim survives, the most recent");

    let changed = service
        .prefer(&owner, None)
        .await
        .expect("clears legacy claims");
    assert_eq!(
        changed.len(),
        1,
        "only the previously visible winner changed"
    );
    assert_eq!(changed[0].id, newer.id);
    assert!(!changed[0].preferred);
    assert!(
        store
            .profiles
            .lock()
            .await
            .values()
            .all(|profile| !profile.preferred),
        "raw legacy claims are all cleared without returning hidden rows"
    );
}
