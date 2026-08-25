//! Tests for environment profile ownership, enforcement, and resolution.
//!
//! Kept in a sibling file so `environment_service.rs` stays within the
//! project's file-size limit while the behaviour remains covered.

use super::*;
use crate::authz::Authorization;
use crate::environment::EnvironmentVariable;
use crate::ids::SubjectId;
use crate::test_support::{environments, profile_spec, FakeProfileStore};

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
    assert_eq!(updated.created_at, created.created_at);
    assert_eq!(updated.name, "second");
    assert_eq!(updated.variables.len(), 1);
    assert_eq!(updated.variables[0].key, "TWO");
}

#[tokio::test]
async fn echoing_normalized_contents_keeps_the_last_modification_time() {
    let store = Arc::new(FakeProfileStore::default());
    let service = environments(Arc::clone(&store));
    let owner = caller("subject-a");
    let created = service
        .create(&owner, profile_spec("unchanged"))
        .await
        .expect("creates");
    let mut historical = created.clone();
    historical.updated_at = "2025-01-01T00:00:00Z".into();
    store
        .upsert(&historical)
        .await
        .expect("seeds old timestamp");

    let echoed = service
        .update(&owner, &historical.id, historical.to_spec())
        .await
        .expect("an echo succeeds");

    assert_eq!(echoed.updated_at, historical.updated_at);
    assert_eq!(echoed.to_spec(), historical.to_spec());
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

/*
 * Which profile an owner's sessions start from — #371.
 *
 * At most one of theirs holds it, and none is a real answer rather than a
 * missing one: it means they have not chosen, and their sessions start from
 * the deployment's own defaults.
 */

#[tokio::test]
async fn a_new_profile_is_not_its_owners_default() {
    // Authoring a profile must not move where every later session starts.
    let service = environments(Arc::new(FakeProfileStore::default()));
    let owner = caller("subject-a");

    let created = service
        .create(&owner, profile_spec("build tools"))
        .await
        .expect("creates");

    assert!(!created.preferred);
}

#[tokio::test]
async fn choosing_a_default_clears_the_one_before_it() {
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
async fn clearing_the_default_is_a_choice_rather_than_an_omission() {
    // It returns the owner to the deployment's own defaults, which is
    // something somebody decides rather than a state they fall into.
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
async fn a_default_survives_an_edit_of_the_profile_holding_it() {
    // Editing a profile is not choosing where sessions start from, and
    // clearing it silently would move that the next time somebody fixed a typo.
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
async fn a_profile_somebody_else_owns_cannot_be_made_their_default() {
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
async fn two_profiles_claiming_the_default_are_settled_on_read() {
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
