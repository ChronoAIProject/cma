use std::sync::Arc;

use crate::environment::AgentConfiguration;
use crate::event::{Event, EventKind, StoredEvent};
use crate::session::{SessionStatus, StopReason};
use crate::test_support::{
    await_settled, environments_defaulting_to, service_with_environments_and_settings, spec, user,
    FakeAdapter, FakeStore,
};
use crate::{PlatformSettings, Rate, SessionSize, Spend};

const CREATED: &str = "2026-01-01T01:00:00Z";

fn priced() -> PlatformSettings {
    let mut settings = PlatformSettings::default();
    settings.pricing = settings.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        },
        CREATED,
    );
    settings
}

fn environments() -> Arc<crate::EnvironmentProfileService> {
    environments_defaulting_to(AgentConfiguration {
        kernel: crate::test_support::kernel(),
        size: Some(SessionSize::new(1_000, 1_024).expect("test size is non-zero")),
        ..Default::default()
    })
}

fn status(
    id: &crate::SessionId,
    seq: u64,
    at: &str,
    value: SessionStatus,
    reason: Option<StopReason>,
) -> StoredEvent {
    StoredEvent::from(Event {
        session_id: id.clone(),
        seq,
        kind: EventKind::StatusChanged {
            status: value,
            stop_reason: reason,
            detail: None,
            capability: None,
        },
        appended_at: at.into(),
    })
}

async fn replace_with_closed_hour(store: &FakeStore, id: &crate::SessionId) {
    store
        .sessions
        .lock()
        .await
        .get_mut(id.as_str())
        .expect("session record")
        .session
        .created_at = CREATED.into();
    *store.events.lock().await = vec![
        status(id, 1, CREATED, SessionStatus::Ready, None),
        status(
            id,
            2,
            "2026-01-01T02:00:00Z",
            SessionStatus::Idle,
            Some(StopReason::UserTerminated),
        ),
    ];
}

#[tokio::test]
async fn two_services_and_later_price_cards_read_the_same_pinned_spend() {
    let store = Arc::new(FakeStore::default());
    let first_settings = priced();
    let first = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments(),
        first_settings.clone(),
    );
    let owner = user("owner");
    let session = first
        .create_session(&owner, spec())
        .await
        .expect("session creates");
    await_settled(&first, &owner, &session.id).await;
    replace_with_closed_hour(&store, &session.id).await;

    let before = first
        .get_spend(&owner, &session.id)
        .await
        .expect("first service folds");

    let mut later_settings = first_settings;
    later_settings.pricing = later_settings.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 90_000,
            memory_micros_per_gib_hour: 10_000,
        },
        "2026-02-01T01:00:00Z",
    );
    later_settings.pricing = later_settings.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 120_000,
            memory_micros_per_gib_hour: 20_000,
        },
        "2026-03-01T01:00:00Z",
    );
    let restarted = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments(),
        later_settings,
    );
    let after = restarted
        .get_spend(&owner, &session.id)
        .await
        .expect("restarted service folds");

    assert_eq!(before, after);
    assert_eq!(before.accrued_micros, Some(45_000));
    assert_eq!(before.billable_seconds, Some(3_600));
    assert!(!before.accruing);
}

#[tokio::test]
async fn an_unpriced_session_reports_absence_and_never_zero() {
    let store = Arc::new(FakeStore::default());
    let service = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments(),
        PlatformSettings::default(),
    );
    let owner = user("owner");
    let session = service
        .create_session(&owner, spec())
        .await
        .expect("unpriced session creates");
    await_settled(&service, &owner, &session.id).await;

    assert_eq!(
        service
            .get_spend(&owner, &session.id)
            .await
            .expect("unpriced spend reads"),
        Spend {
            priced: false,
            rate_card: None,
            micros_per_hour: None,
            accrued_micros: None,
            billable_seconds: None,
            accruing: false,
            unbounded_intervals: 0,
        }
    );
}

#[tokio::test]
async fn another_owner_cannot_read_spend() {
    let store = Arc::new(FakeStore::default());
    let service = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments(),
        priced(),
    );
    let owner = user("owner");
    let session = service
        .create_session(&owner, spec())
        .await
        .expect("session creates");

    assert_eq!(
        service.get_spend(&user("somebody-else"), &session.id).await,
        Err(crate::CoreError::Forbidden)
    );
}
