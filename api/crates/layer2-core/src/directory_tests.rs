//! Tests for the record of who has used the service.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::authz::{Authorization, Caller, Capability};
use crate::directory::{DirectoryQuery, RegisteredUser, UserDirectory, MAX_DIRECTORY_LIMIT};
use crate::error::CoreError;
use crate::ids::SubjectId;
use crate::store::{StoreError, UserDirectoryStore};

/// A directory held in memory, preserving `first_seen` exactly as a real one
/// must — that preservation is the behaviour most of these tests are about.
#[derive(Default)]
struct FakeDirectoryStore {
    users: Mutex<HashMap<String, RegisteredUser>>,
    broken: bool,
    /// The limit the last listing reached the store with, so a bound applied
    /// only after reading everything cannot pass for one pushed into the query.
    asked_for: Mutex<Option<usize>>,
    /// The subjects the last naming asked about, for the same reason.
    named: Mutex<Option<Vec<SubjectId>>>,
}

#[async_trait::async_trait]
impl UserDirectoryStore for FakeDirectoryStore {
    async fn record_sign_in(&self, user: &RegisteredUser) -> Result<(), StoreError> {
        if self.broken {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        let mut users = self.users.lock().await;
        let entry = users
            .entry(user.subject.0.clone())
            .or_insert_with(|| user.clone());
        entry.display_name = user.display_name.clone();
        entry.email = user.email.clone();
        entry.last_seen = user.last_seen.clone();
        Ok(())
    }

    async fn recent(&self, query: &DirectoryQuery) -> Result<Vec<RegisteredUser>, StoreError> {
        if self.broken {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        *self.asked_for.lock().await = Some(query.limit);
        let mut out: Vec<RegisteredUser> = self.users.lock().await.values().cloned().collect();
        out.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        out.truncate(query.limit);
        Ok(out)
    }

    async fn by_subjects(&self, subjects: &[SubjectId]) -> Result<Vec<RegisteredUser>, StoreError> {
        if self.broken {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        *self.named.lock().await = Some(subjects.to_vec());
        let users = self.users.lock().await;
        Ok(subjects
            .iter()
            .filter_map(|s| users.get(&s.0).cloned())
            .collect())
    }
}

fn operator() -> Caller {
    Caller::new(
        SubjectId("operator".into()),
        Authorization::from_capabilities([Capability::ReadUserDirectory]),
    )
}

fn ordinary() -> Caller {
    Caller::new(
        SubjectId("alice".into()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::ReadAllSessions,
        ]),
    )
}

#[tokio::test]
async fn signing_in_records_a_person_an_operator_can_then_see() {
    let store = Arc::new(FakeDirectoryStore::default());
    let directory = UserDirectory::new(Arc::clone(&store) as Arc<dyn UserDirectoryStore>);

    directory
        .note_sign_in(
            &SubjectId("subject-a".into()),
            Some("Ada Lovelace".into()),
            Some("ada@example.com".into()),
        )
        .await;

    let users = directory
        .list(&operator(), DirectoryQuery::default())
        .await
        .expect("readable");
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].subject, SubjectId("subject-a".into()));
    assert_eq!(users[0].label(), Some("Ada Lovelace"));
}

#[tokio::test]
async fn signing_in_again_keeps_the_first_time_they_were_seen() {
    // The one fact this record exists to hold. An upsert that overwrote it
    // would make everybody look like they arrived this morning.
    let store = Arc::new(FakeDirectoryStore::default());
    let directory = UserDirectory::new(Arc::clone(&store) as Arc<dyn UserDirectoryStore>);
    let subject = SubjectId("subject-a".into());

    directory
        .note_sign_in(&subject, Some("Ada".into()), None)
        .await;
    let first = directory
        .list(&operator(), DirectoryQuery::default())
        .await
        .expect("readable")[0]
        .clone();

    directory
        .note_sign_in(&subject, Some("Ada Lovelace".into()), None)
        .await;
    let again = directory
        .list(&operator(), DirectoryQuery::default())
        .await
        .expect("readable")[0]
        .clone();

    assert_eq!(again.first_seen, first.first_seen);
    assert_eq!(
        again.display_name.as_deref(),
        Some("Ada Lovelace"),
        "a name the provider has changed must be picked up"
    );
}

#[tokio::test]
async fn an_ordinary_caller_cannot_read_the_directory() {
    let store = Arc::new(FakeDirectoryStore::default());
    let directory = UserDirectory::new(store as Arc<dyn UserDirectoryStore>);
    assert_eq!(
        directory
            .list(&ordinary(), DirectoryQuery::default())
            .await
            .err(),
        Some(CoreError::Forbidden),
        "who else uses the service is not an ordinary caller's business"
    );
}

#[tokio::test]
async fn a_directory_that_cannot_be_reached_is_reported_rather_than_read_as_empty() {
    let store = Arc::new(FakeDirectoryStore {
        broken: true,
        ..FakeDirectoryStore::default()
    });
    let directory = UserDirectory::new(store as Arc<dyn UserDirectoryStore>);

    let err = directory
        .list(&operator(), DirectoryQuery::default())
        .await
        .expect_err("an unreachable store must not read as an empty directory");
    assert!(
        matches!(err, CoreError::RuntimeUnavailable(_)),
        "got {err:?}"
    );
}

#[tokio::test]
async fn a_sign_in_survives_a_directory_that_is_down() {
    // Nobody is turned away from the service because the note about them could
    // not be written.
    let store = Arc::new(FakeDirectoryStore {
        broken: true,
        ..FakeDirectoryStore::default()
    });
    let directory = UserDirectory::new(Arc::clone(&store) as Arc<dyn UserDirectoryStore>);
    directory
        .note_sign_in(&SubjectId("subject-a".into()), None, None)
        .await;
    assert!(store.users.lock().await.is_empty());
}

#[test]
fn a_person_is_named_by_their_account_and_only_then_by_their_email() {
    let both = RegisteredUser::seen_now(
        SubjectId("subject-a".into()),
        Some("Ada Lovelace".into()),
        Some("ada@example.com".into()),
    );
    assert_eq!(both.label(), Some("Ada Lovelace"));

    let email_only = RegisteredUser::seen_now(
        SubjectId("subject-a".into()),
        None,
        Some("ada@example.com".into()),
    );
    assert_eq!(email_only.label(), Some("ada@example.com"));
}

#[test]
fn a_provider_that_released_nothing_leaves_nothing_to_show() {
    // `None` is the honest answer, and it is what stops a surface printing an
    // opaque identifier at somebody as though it were their name.
    let anonymous = RegisteredUser::seen_now(SubjectId("76fe9d91".into()), None, None);
    assert_eq!(anonymous.label(), None);

    // A provider returning whitespace has not given a name either; treating it
    // as one produces a label that reads as a rendering fault.
    let blank = RegisteredUser::seen_now(
        SubjectId("76fe9d91".into()),
        Some("   ".into()),
        Some("".into()),
    );
    assert_eq!(blank.label(), None);
    assert!(blank.display_name.is_none());
    assert!(blank.email.is_none());
}

#[test]
fn a_person_serialises_without_null_fields_a_client_must_special_case() {
    let user = RegisteredUser::seen_now(
        SubjectId("subject-a".into()),
        None,
        Some("ada@example.com".into()),
    );
    let json = serde_json::to_value(&user).expect("serialises");
    assert!(json.get("display_name").is_none(), "got {json}");
    assert_eq!(json["email"], "ada@example.com");
    assert_eq!(json["subject"], "subject-a");
    assert!(json["first_seen"].as_str().expect("set").ends_with('Z'));
}

#[tokio::test]
async fn a_listing_reaches_the_store_with_a_bound_on_it() {
    // The directory only ever grows, and a read with no ceiling works right up
    // until it does not. Asserted against what the store was asked for, since
    // truncating the answer afterwards would pass every test about the answer
    // while leaving the cost exactly where it was.
    let store = Arc::new(FakeDirectoryStore::default());
    let directory = UserDirectory::new(Arc::clone(&store) as Arc<dyn UserDirectoryStore>);

    directory
        .list(&operator(), DirectoryQuery::default())
        .await
        .expect("readable");
    assert_eq!(
        *store.asked_for.lock().await,
        Some(crate::directory::DEFAULT_DIRECTORY_LIMIT)
    );

    directory
        .list(&operator(), DirectoryQuery { limit: 10_000 })
        .await
        .expect("readable");
    assert_eq!(
        *store.asked_for.lock().await,
        Some(MAX_DIRECTORY_LIMIT),
        "a caller must not be able to ask for the whole table"
    );

    directory
        .list(&operator(), DirectoryQuery { limit: 0 })
        .await
        .expect("readable");
    assert_eq!(
        *store.asked_for.lock().await,
        Some(1),
        "a zero would ask the store for nothing at all"
    );
}

#[tokio::test]
async fn naming_a_page_asks_only_about_the_people_on_it() {
    // This runs on every read of the sessions view, so resolving names by
    // reading the whole directory would make one bounded page cost more every
    // day the service runs.
    let store = Arc::new(FakeDirectoryStore::default());
    let directory = UserDirectory::new(Arc::clone(&store) as Arc<dyn UserDirectoryStore>);
    for subject in ["subject-a", "subject-b", "subject-c"] {
        directory
            .note_sign_in(
                &SubjectId(subject.into()),
                Some("Ada Lovelace".into()),
                None,
            )
            .await;
    }

    let wanted = vec![SubjectId("subject-b".into())];
    let names = directory
        .names_known_to(&operator(), &wanted)
        .await
        .expect("readable");

    assert_eq!(*store.named.lock().await, Some(wanted));
    assert_eq!(names.len(), 1);
    assert_eq!(
        names
            .get(&SubjectId("subject-b".into()))
            .map(String::as_str),
        Some("Ada Lovelace")
    );
}

#[tokio::test]
async fn a_caller_who_may_not_read_the_directory_gets_no_names_rather_than_no_page() {
    // Each view is offered on its own capability. Refusing the list of work
    // because the *second* grant is missing would leave a view drawn empty by
    // entitlement, and an empty list has to keep meaning "there is nothing".
    let store = Arc::new(FakeDirectoryStore::default());
    let directory = UserDirectory::new(Arc::clone(&store) as Arc<dyn UserDirectoryStore>);
    directory
        .note_sign_in(&SubjectId("subject-a".into()), Some("Ada".into()), None)
        .await;

    let names = directory
        .names_known_to(&ordinary(), &[SubjectId("subject-a".into())])
        .await
        .expect("a caller without the grant is not refused, only unaided");
    assert!(names.is_empty(), "a name is not theirs to see");
    assert_eq!(
        *store.named.lock().await,
        None,
        "and the store must not have been asked at all"
    );
}

#[tokio::test]
async fn naming_nobody_asks_the_store_nothing() {
    let store = Arc::new(FakeDirectoryStore::default());
    let directory = UserDirectory::new(Arc::clone(&store) as Arc<dyn UserDirectoryStore>);
    let names = directory
        .names_known_to(&operator(), &[])
        .await
        .expect("readable");
    assert!(names.is_empty());
    assert_eq!(*store.named.lock().await, None);
}
