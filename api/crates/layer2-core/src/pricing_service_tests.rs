//! Quoting and pinning prices at the session creation boundary.

use std::sync::{Arc, Mutex};

use crate::authz::{Authorization, Caller, Capability};
use crate::environment::AgentConfiguration;
use crate::store::{PlatformSettingsStore, StoreError, StoredSettings};
use crate::test_support::{
    environments_defaulting_to, profile_spec, service_with_environments_and_settings,
    service_with_environments_and_settings_service, spec, user, FakeAdapter, FakeStore,
};
use crate::{CoreError, PlatformSettings, PlatformSettingsService, Rate, SessionSize};

struct MutableSettings(Mutex<StoredSettings>);

impl MutableSettings {
    fn new(settings: PlatformSettings) -> Self {
        Self(Mutex::new(StoredSettings {
            settings,
            revision: 1,
            previous: None,
        }))
    }
}

#[async_trait::async_trait]
impl PlatformSettingsStore for MutableSettings {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        Ok(Some(self.0.lock().expect("test settings lock").clone()))
    }

    async fn save_if_unchanged(
        &self,
        expected: u64,
        settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        let mut stored = self.0.lock().expect("test settings lock");
        if stored.revision != expected {
            return Ok(None);
        }
        let next = StoredSettings {
            settings: settings.clone(),
            revision: expected + 1,
            previous: Some(stored.settings.clone()),
        };
        *stored = next.clone();
        Ok(Some(next))
    }
}

struct BrokenSettings;

#[async_trait::async_trait]
impl PlatformSettingsStore for BrokenSettings {
    async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
        Err(StoreError::Unavailable("settings cannot be read".into()))
    }

    async fn save_if_unchanged(
        &self,
        _expected: u64,
        _settings: &PlatformSettings,
    ) -> Result<Option<StoredSettings>, StoreError> {
        Err(StoreError::Unavailable("settings cannot be written".into()))
    }
}

fn sized_agent() -> AgentConfiguration {
    AgentConfiguration {
        kernel: crate::test_support::kernel(),
        size: Some(SessionSize::new(1_000, 1_024).expect("test size is non-zero")),
        ..Default::default()
    }
}

fn priced() -> PlatformSettings {
    let mut settings = PlatformSettings::default();
    settings.pricing = settings.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        },
        "2026-08-23T01:00:00Z",
    );
    settings
}

#[tokio::test]
async fn the_quoted_integer_is_the_integer_pinned_on_creation() {
    let store = Arc::new(FakeStore::default());
    let service = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments_defaulting_to(sized_agent()),
        priced(),
    );
    let owner = user("owner");

    let quote = service
        .quote(&owner, None)
        .await
        .expect("quote succeeds")
        .expect("deployment is priced");
    let mut request = spec();
    request.quoted_rate_card = Some(quote.rate_card.clone());
    let session = service
        .create_session(&owner, request)
        .await
        .expect("the accepted quote creates");
    let pinned = session.pricing.expect("the price is pinned");

    assert_eq!(quote.micros_per_hour, 45_000);
    assert_eq!(pinned.micros_per_hour, quote.micros_per_hour);
    assert_eq!(pinned.rate_card, quote.rate_card);
    assert_eq!(crate::format_usd(pinned.micros_per_hour), quote.display);
    assert_eq!(pinned.cpu_millis, quote.cpu_millis);
    assert_eq!(pinned.memory_mib, quote.memory_mib);
}

#[tokio::test]
async fn an_explicit_profile_is_quoted_at_the_size_creation_uses() {
    let owner = user("owner");
    let environments = environments_defaulting_to(AgentConfiguration {
        kernel: crate::test_support::kernel(),
        size: None,
        ..Default::default()
    });
    let mut authored = profile_spec("sized");
    authored.agent.size = Some(SessionSize::new(250, 2_048).expect("test size is non-zero"));
    let profile = environments
        .create(&owner, authored)
        .await
        .expect("profile creates");
    let service = service_with_environments_and_settings(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
        environments,
        priced(),
    );

    let quote = service
        .quote(&owner, Some(&profile.id))
        .await
        .expect("quote succeeds")
        .expect("deployment is priced");
    let mut request = spec();
    request.environment_profile = Some(profile.id);
    request.quoted_rate_card = Some(quote.rate_card.clone());
    let created = service
        .create_session(&owner, request)
        .await
        .expect("quoted profile creates");
    let pinned = created.pricing.expect("price is pinned");

    assert_eq!((quote.cpu_millis, quote.memory_mib), (250, 2_048));
    assert_eq!((pinned.cpu_millis, pinned.memory_mib), (250, 2_048));
    assert_eq!(pinned.micros_per_hour, quote.micros_per_hour);
}

#[tokio::test]
async fn a_retired_quoted_card_is_refused_before_any_record_exists() {
    let first = priced();
    let quoted = first.pricing.in_force().expect("first card").id.clone();
    let mut current = first;
    current.pricing = current.pricing.appended(
        Rate {
            cpu_micros_per_cpu_hour: 50_000,
            memory_micros_per_gib_hour: 6_000,
        },
        "2026-08-23T02:00:00Z",
    );
    let in_force = current.pricing.in_force().expect("new card").id.clone();
    let store = Arc::new(FakeStore::default());
    let service = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments_defaulting_to(sized_agent()),
        current,
    );
    let mut request = spec();
    request.quoted_rate_card = Some(quoted.clone());

    let error = service
        .create_session(&user("owner"), request)
        .await
        .expect_err("a retired quote must fail");

    assert_eq!(
        error,
        CoreError::PriceChanged {
            quoted: quoted.0,
            in_force: in_force.0,
        }
    );
    assert!(store.sessions.lock().await.is_empty());
}

#[tokio::test]
async fn omitting_the_quoted_card_never_refuses_and_pins_the_current_one() {
    let settings = priced();
    let current = settings.pricing.in_force().expect("card").id.clone();
    let service = service_with_environments_and_settings(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
        environments_defaulting_to(sized_agent()),
        settings,
    );

    let session = service
        .create_session(&user("owner"), spec())
        .await
        .expect("an old client still creates");

    assert_eq!(session.pricing.expect("priced").rate_card, current);
}

#[tokio::test]
async fn editing_the_briefing_five_times_does_not_retire_a_quote() {
    let backing = Arc::new(MutableSettings::new(priced()));
    let settings = Arc::new(PlatformSettingsService::new(
        Arc::clone(&backing) as Arc<dyn PlatformSettingsStore>
    ));
    let admin = Caller::new(
        crate::SubjectId("admin".into()),
        Authorization::from_capabilities([Capability::ConfigurePlatform]),
    );
    let original = settings.read(&admin).await.expect("settings read");
    let quoted = original
        .settings
        .pricing
        .in_force()
        .expect("price card")
        .id
        .clone();
    let mut revision = original.revision;
    for edit in 1..=5 {
        let mut changed = settings.read(&admin).await.expect("settings read").settings;
        changed.briefing.before = vec![format!("briefing edit {edit}")];
        revision = settings
            .write(&admin, revision, changed)
            .await
            .expect("prose edit writes")
            .revision;
    }
    assert_eq!(revision, original.revision + 5);

    let service = service_with_environments_and_settings_service(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
        environments_defaulting_to(sized_agent()),
        settings,
    );
    let mut request = spec();
    request.quoted_rate_card = Some(quoted.clone());

    let created = service
        .create_session(&user("owner"), request)
        .await
        .expect("a prose revision did not change the price");
    assert_eq!(created.pricing.expect("priced").rate_card, quoted);
}

#[tokio::test]
async fn an_unreadable_settings_store_records_the_session_unpriced() {
    let settings = Arc::new(PlatformSettingsService::new(Arc::new(BrokenSettings)));
    let store = Arc::new(FakeStore::default());
    let service = service_with_environments_and_settings_service(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments_defaulting_to(sized_agent()),
        settings,
    );
    let mut request = spec();
    request.quoted_rate_card = Some(crate::RateCardId("rate_from_quote".into()));

    let created = service
        .create_session(&user("owner"), request)
        .await
        .expect("settings fallback keeps creation available");

    assert_eq!(created.pricing, None);
    assert_eq!(store.sessions.lock().await.len(), 1);
    assert_eq!(
        store
            .sessions
            .lock()
            .await
            .get(created.id.as_str())
            .expect("the record exists")
            .session
            .pricing,
        None
    );
}

#[tokio::test]
async fn no_price_is_absence_not_zero_and_an_unsized_session_still_creates() {
    let service = service_with_environments_and_settings(
        Arc::new(FakeStore::default()),
        Arc::new(FakeAdapter::default()),
        environments_defaulting_to(AgentConfiguration {
            kernel: crate::test_support::kernel(),
            size: None,
            ..Default::default()
        }),
        PlatformSettings::default(),
    );
    let owner = user("owner");

    assert_eq!(
        service.quote(&owner, None).await.expect("quote works"),
        None
    );
    assert_eq!(
        service
            .create_session(&owner, spec())
            .await
            .expect("unpriced remains usable")
            .pricing,
        None
    );
}

#[tokio::test]
async fn a_priced_session_without_a_declared_size_is_refused_by_name() {
    let store = Arc::new(FakeStore::default());
    let service = service_with_environments_and_settings(
        Arc::clone(&store),
        Arc::new(FakeAdapter::default()),
        environments_defaulting_to(AgentConfiguration {
            kernel: crate::test_support::kernel(),
            size: None,
            ..Default::default()
        }),
        priced(),
    );
    let owner = user("owner");

    for error in [
        service
            .quote(&owner, None)
            .await
            .expect_err("quote must fail"),
        service
            .create_session(&owner, spec())
            .await
            .expect_err("creation must fail"),
    ] {
        assert_eq!(error.code(), "invalid_request");
        assert!(error.to_string().contains("assumed session size"));
        assert!(error.to_string().contains("administrator"));
    }
    assert!(store.sessions.lock().await.is_empty());
}
