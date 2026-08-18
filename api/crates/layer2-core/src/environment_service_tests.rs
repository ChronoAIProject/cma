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
    assert_eq!(service.list(&owner).await.expect("lists").len(), 1);
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

    let mine = service.list(&caller("subject-a")).await.expect("lists");
    assert_eq!(mine.len(), 2);
    assert!(mine.iter().all(|p| p.owner.as_str() == "subject-a"));
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
        service.list(&unarmed).await.expect_err("refused"),
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
    assert!(service.list(&owner).await.expect("lists").is_empty());
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
