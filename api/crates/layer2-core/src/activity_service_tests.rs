//! Tests for keeping and reading the record of what was asked for.
//!
//! Written against a store held in memory, because everything worth asserting
//! here is about what reaches the store and who is allowed to read it back —
//! neither of which needs a database to be true.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::activity::{
    ActivityAction, ActivityOutcome, ActivityQuery, ActivityRecord, ActivityTarget,
    MAX_ACTIVITY_LIMIT,
};
use crate::activity_detail::DetailKey;
use crate::activity_service::ActivityService;
use crate::authz::{Authorization, Caller, Capability};
use crate::error::CoreError;
use crate::ids::{SessionId, SubjectId};
use crate::store::{ActivityStore, StoreError};

/// A store held in memory, newest first, so the query semantics can be checked.
#[derive(Default)]
struct FakeActivityStore {
    entries: Mutex<Vec<ActivityRecord>>,
    /// When set, every operation fails, so a reader can be checked against a
    /// store that is down rather than merely empty.
    broken: bool,
    /// The limit the last read asked for, so the clamp can be checked where it
    /// matters — in what the database is told, not in what is returned.
    asked_for: Mutex<Option<usize>>,
}

#[async_trait::async_trait]
impl ActivityStore for FakeActivityStore {
    async fn append(&self, record: &ActivityRecord) -> Result<(), StoreError> {
        if self.broken {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        self.entries.lock().await.insert(0, record.clone());
        Ok(())
    }

    async fn recent(&self, query: &ActivityQuery) -> Result<Vec<ActivityRecord>, StoreError> {
        *self.asked_for.lock().await = Some(query.limit);
        if self.broken {
            return Err(StoreError::Unavailable("test store is down".into()));
        }
        Ok(self
            .entries
            .lock()
            .await
            .iter()
            .filter(|e| match &query.actor {
                Some(actor) => e.actor.as_ref() == Some(actor),
                None => true,
            })
            .take(query.limit)
            .cloned()
            .collect())
    }
}

fn operator() -> Caller {
    Caller::new(
        SubjectId("operator".into()),
        Authorization::from_capabilities([Capability::ReadActivityLog]),
    )
}

fn ordinary() -> Caller {
    Caller::new(
        SubjectId("alice".into()),
        Authorization::from_capabilities([
            Capability::CreateSession,
            Capability::UseOwnSessions,
            Capability::TerminateOwnSession,
            Capability::ManageOwnEnvironmentProfiles,
            Capability::ReadAllSessions,
        ]),
    )
}

#[tokio::test]
async fn a_recorded_action_can_be_read_back_by_an_operator() {
    let store = Arc::new(FakeActivityStore::default());
    let service = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);

    assert!(
        service
            .record(
                ActivityRecord::new(ActivityAction::SessionTerminate, ActivityOutcome::Succeeded)
                    .by(SubjectId("operator".into()))
                    .on(ActivityTarget::Session(SessionId("sesn_1".into()))),
            )
            .await
    );

    let entries = service
        .list(&operator(), ActivityQuery::default())
        .await
        .expect("an operator may read the record");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, ActivityAction::SessionTerminate);
    assert_eq!(
        entries[0].target,
        Some(ActivityTarget::Session(SessionId("sesn_1".into())))
    );
}

#[tokio::test]
async fn an_ordinary_caller_cannot_read_the_record_at_all() {
    // Not even their own entries. The record names other subjects and says
    // what the service is doing for them, and there is no filter that makes
    // that safe to hand out.
    let store = Arc::new(FakeActivityStore::default());
    let service = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);
    service
        .record(
            ActivityRecord::new(ActivityAction::SessionCreate, ActivityOutcome::Succeeded)
                .by(SubjectId("alice".into())),
        )
        .await;

    assert_eq!(
        service
            .list(&ordinary(), ActivityQuery::default())
            .await
            .err(),
        Some(CoreError::Forbidden),
        "everyday capabilities, including reading every session, must not open this"
    );
}

#[tokio::test]
async fn a_reader_can_narrow_to_one_subject() {
    let store = Arc::new(FakeActivityStore::default());
    let service = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);
    for subject in ["alice", "bob", "alice"] {
        service
            .record(
                ActivityRecord::new(ActivityAction::SessionCreate, ActivityOutcome::Succeeded)
                    .by(SubjectId(subject.into())),
            )
            .await;
    }

    let entries = service
        .list(
            &operator(),
            ActivityQuery {
                actor: Some(SubjectId("alice".into())),
                ..ActivityQuery::default()
            },
        )
        .await
        .expect("readable");
    assert_eq!(entries.len(), 2);
    assert!(entries
        .iter()
        .all(|e| e.actor == Some(SubjectId("alice".into()))));
}

#[tokio::test]
async fn a_read_never_returns_more_than_the_log_will_serve() {
    let store = Arc::new(FakeActivityStore::default());
    let service = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);
    for _ in 0..3 {
        service
            .record(ActivityRecord::new(
                ActivityAction::SessionList,
                ActivityOutcome::Succeeded,
            ))
            .await;
    }

    // The clamp is what the store is asked for, so a caller cannot turn one
    // request into a full table scan by naming a large enough number.
    let entries = service
        .list(
            &operator(),
            ActivityQuery {
                actor: None,
                limit: usize::MAX,
            },
        )
        .await
        .expect("readable");
    assert_eq!(entries.len(), 3);
    assert_eq!(
        *store.asked_for.lock().await,
        Some(MAX_ACTIVITY_LIMIT),
        "the clamp has to reach the store, or a caller can ask a database for everything"
    );
}

#[tokio::test]
async fn a_store_that_cannot_be_reached_is_reported_rather_than_read_as_empty() {
    // An empty answer would say nothing has happened, which is the one wrong
    // thing this surface can tell somebody investigating an incident.
    let store = Arc::new(FakeActivityStore {
        broken: true,
        ..FakeActivityStore::default()
    });
    let service = ActivityService::new(store as Arc<dyn ActivityStore>);

    let err = service
        .list(&operator(), ActivityQuery::default())
        .await
        .expect_err("an unreachable store must not read as an empty log");
    assert!(
        matches!(err, CoreError::RuntimeUnavailable(_)),
        "got {err:?}"
    );
}

#[tokio::test]
async fn recording_survives_a_store_that_is_down() {
    // The work being described already happened. Failing it because the note
    // about it could not be written would turn a storage problem into an
    // outage, so this must return quietly and log loudly.
    let store = Arc::new(FakeActivityStore {
        broken: true,
        ..FakeActivityStore::default()
    });
    let service = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);

    assert!(
        !service
            .record(ActivityRecord::new(
                ActivityAction::SessionCreate,
                ActivityOutcome::Succeeded,
            ))
            .await
    );
    assert!(store.entries.lock().await.is_empty());
}

#[tokio::test]
async fn a_recorded_action_keeps_only_allowlisted_parameters() {
    // The end-to-end statement of the rule: whatever a caller sent, what is
    // kept is what somebody decided may be kept.
    let store = Arc::new(FakeActivityStore::default());
    let service = ActivityService::new(Arc::clone(&store) as Arc<dyn ActivityStore>);

    service
        .record(
            ActivityRecord::new(
                ActivityAction::EnvironmentProfileCreate,
                ActivityOutcome::Succeeded,
            )
            .by(SubjectId("alice".into()))
            .with(DetailKey::ResourceName, "build tools")
            .with(DetailKey::VariableCount, "3"),
        )
        .await;

    let entries = service
        .list(&operator(), ActivityQuery::default())
        .await
        .expect("readable");
    let rendered = serde_json::to_string(&entries[0]).expect("serialises");
    assert!(rendered.contains("variable_count"), "got {rendered}");
    assert!(
        !rendered.contains("value"),
        "a bundle's values have no key and must never appear: {rendered}"
    );
}
