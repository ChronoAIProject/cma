//! Reading and changing the deployment's own settings.
//!
//! Two callers with different rights, deliberately kept apart:
//!
//! - an **administrator**, who must hold [`Capability::ConfigurePlatform`] and
//!   whose every read and write is checked here rather than at whatever
//!   interface asked;
//! - **provisioning**, which has no caller at all — a session may be brought
//!   back by reconciliation with nobody on the line — and reads through a
//!   separate method that takes no `Caller` and can therefore never be handed
//!   one to check.
//!
//! Splitting them is what stops the second quietly becoming an unauthenticated
//! route to the first. A method that takes an `Option<&Caller>` and skips the
//! check when it is `None` is one refactor away from being called with `None`
//! from somewhere that had a caller.

use std::sync::Arc;

use crate::authz::Caller;
use crate::authz::Capability;
use crate::error::CoreError;
use crate::store::{PlatformSettingsStore, StoredSettings};
use crate::{PlatformSettings, SessionBriefing};

/// The deployment's settings, and who may change them.
pub struct PlatformSettingsService {
    store: Option<Arc<dyn PlatformSettingsStore>>,
}

impl PlatformSettingsService {
    /// With a store behind it.
    pub fn new(store: Arc<dyn PlatformSettingsStore>) -> Self {
        Self { store: Some(store) }
    }

    /// With none.
    ///
    /// A deployment that configures no store still runs, and every session
    /// gets the shipped defaults. Reads answer with them; writes are refused
    /// by name rather than accepted and dropped, because a settings page that
    /// says "saved" and changes nothing is worse than one that says it cannot.
    pub fn unconfigured() -> Self {
        Self { store: None }
    }

    /// What the settings are, for an administrator.
    pub async fn read(&self, caller: &Caller) -> Result<StoredSettings, CoreError> {
        caller
            .authorization
            .require(Capability::ConfigurePlatform)?;
        self.current().await
    }

    /// What the settings are, for provisioning, which has no caller.
    ///
    /// Never fails: a store that cannot be read must not stop a session
    /// starting. The shipped defaults are what the service was doing before
    /// any of this existed, so falling back to them is falling back to the
    /// behaviour every session had yesterday — and refusing to start a session
    /// because a settings document could not be fetched would be a far worse
    /// outcome than one briefly reading the shipped words.
    pub async fn for_provisioning(&self) -> PlatformSettings {
        self.for_provisioning_observed().await.0
    }

    /// Provisioning settings together with the failure that caused fallback.
    ///
    /// Kept crate-private because only creation needs to record the permanent
    /// consequence of a transient read failure. Other provisioning readers
    /// need only the never-failing settings value above.
    pub(crate) async fn for_provisioning_observed(&self) -> (PlatformSettings, Option<CoreError>) {
        match self.current().await {
            Ok(stored) => (stored.settings, None),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "the platform's settings could not be read; this session gets the shipped defaults"
                );
                (PlatformSettings::default(), Some(e))
            }
        }
    }

    /// Change the settings, refusing if somebody has written since `expected`.
    pub async fn write(
        &self,
        caller: &Caller,
        expected: u64,
        settings: PlatformSettings,
    ) -> Result<StoredSettings, CoreError> {
        caller
            .authorization
            .require(Capability::ConfigurePlatform)?;
        let Some(store) = self.store.as_ref() else {
            return Err(CoreError::SettingsUnavailable(
                "this deployment stores no platform settings, so they cannot be changed".into(),
            ));
        };
        let current = self.current().await?;
        if current.revision == expected {
            settings
                .pricing
                .sealed_against(&current.settings.pricing)
                .map_err(|violation| CoreError::PriceListNotAppendOnly(violation.to_string()))?;
        }
        match store.save_if_unchanged(expected, &settings).await {
            Ok(Some(stored)) => {
                tracing::info!(
                    subject = %caller.subject,
                    revision = stored.revision,
                    "the platform's settings were changed"
                );
                Ok(stored)
            }
            // Named rather than reported as a failure: nothing is wrong, the
            // caller is simply working from a version somebody has replaced,
            // and the only useful thing to do is tell them so.
            Ok(None) => Err(CoreError::SettingsConflict(
                "these settings were changed by somebody else since you read them".into(),
            )),
            Err(e) => Err(CoreError::SettingsUnavailable(format!(
                "they could not be written: {e}"
            ))),
        }
    }

    /// Put the session briefing back to what the platform ships.
    ///
    /// A write like any other, and conditional like any other. Its own method
    /// because a client should not have to hold a copy of the defaults to
    /// restore them — and if it did, its copy would be the one that drifts.
    /// Other settings are preserved: resetting prose must not quietly widen a
    /// size bound or remove a live-session limit set for a different reason.
    pub async fn reset_briefing(
        &self,
        caller: &Caller,
        expected: u64,
    ) -> Result<StoredSettings, CoreError> {
        let mut current = self.read(caller).await?.settings;
        current.briefing = SessionBriefing::default();
        self.write(caller, expected, current).await
    }

    /// What is stored, or the shipped defaults where nothing is.
    async fn current(&self) -> Result<StoredSettings, CoreError> {
        let Some(store) = self.store.as_ref() else {
            return Ok(Self::shipped());
        };
        match store.load().await {
            Ok(Some(stored)) => Ok(stored),
            // Nothing written is a real answer, not a missing record: it is
            // what every deployment looks like until somebody changes
            // something.
            Ok(None) => Ok(Self::shipped()),
            Err(e) => Err(CoreError::SettingsUnavailable(format!(
                "they could not be read: {e}"
            ))),
        }
    }

    /// The defaults, at revision zero.
    ///
    /// Zero is what an unconfigured deployment reports and what a store holds
    /// nothing at, so a client that saves what it read sends `expected: 0` and
    /// the first write takes the same conditional path as every write after
    /// it — no special case that skips the check, and no way for two clients
    /// to both believe they were first.
    fn shipped() -> StoredSettings {
        StoredSettings {
            settings: PlatformSettings::default(),
            revision: 0,
            previous: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authz::Authorization;
    use crate::ids::SubjectId;
    use crate::platform::SessionBriefing;
    use crate::store::StoreError;
    use crate::Rate;
    use std::sync::Mutex;

    /// A store that holds one record, the way the real one does.
    #[derive(Default)]
    struct FakeStore {
        held: Mutex<Option<StoredSettings>>,
        /// Set to make every call fail, so the read path's fallback and the
        /// write path's refusal can both be exercised.
        broken: bool,
    }

    #[async_trait::async_trait]
    impl PlatformSettingsStore for FakeStore {
        async fn load(&self) -> Result<Option<StoredSettings>, StoreError> {
            if self.broken {
                return Err(StoreError::Unavailable("no".into()));
            }
            Ok(self.held.lock().expect("not poisoned").clone())
        }

        async fn save_if_unchanged(
            &self,
            expected: u64,
            settings: &PlatformSettings,
        ) -> Result<Option<StoredSettings>, StoreError> {
            if self.broken {
                return Err(StoreError::Unavailable("no".into()));
            }
            let mut held = self.held.lock().expect("not poisoned");
            let at = held.as_ref().map_or(0, |s| s.revision);
            if at != expected {
                return Ok(None);
            }
            let next = StoredSettings {
                settings: settings.clone(),
                revision: at + 1,
                previous: held.as_ref().map(|s| s.settings.clone()),
            };
            *held = Some(next.clone());
            Ok(Some(next))
        }
    }

    fn admin() -> Caller {
        Caller::new(
            SubjectId("admin".into()),
            Authorization::from_capabilities([Capability::ConfigurePlatform]),
        )
    }

    fn ordinary() -> Caller {
        Caller::new(
            SubjectId("someone".into()),
            Authorization::from_capabilities([
                Capability::CreateSession,
                Capability::UseOwnSessions,
                // Every operator read, and still not this.
                Capability::ReadAllSessions,
                Capability::ReadActivityLog,
                Capability::ControlAnySession,
            ]),
        )
    }

    fn changed() -> PlatformSettings {
        PlatformSettings {
            briefing: SessionBriefing {
                before: vec!["# Read this first".into()],
                ..SessionBriefing::default()
            },
            ..PlatformSettings::default()
        }
    }

    fn rate() -> Rate {
        Rate {
            cpu_micros_per_cpu_hour: 40_000,
            memory_micros_per_gib_hour: 5_000,
        }
    }

    #[tokio::test]
    async fn an_unconfigured_deployment_reads_the_shipped_words() {
        // The common case, and it must not look like a failure: a deployment
        // nobody has configured is every deployment on its first day.
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        let stored = svc.read(&admin()).await.expect("readable");
        assert_eq!(stored.settings, PlatformSettings::default());
        assert_eq!(stored.revision, 0);
    }

    #[tokio::test]
    async fn only_a_caller_who_holds_the_capability_may_read_or_write() {
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        assert!(matches!(
            svc.read(&ordinary()).await,
            Err(CoreError::Forbidden)
        ));
        assert!(matches!(
            svc.write(&ordinary(), 0, changed()).await,
            Err(CoreError::Forbidden)
        ));
        assert!(matches!(
            svc.reset_briefing(&ordinary(), 0).await,
            Err(CoreError::Forbidden)
        ));
        // And the refusal is not a side effect of the store being empty.
        assert!(svc.read(&admin()).await.is_ok());
    }

    #[tokio::test]
    async fn a_write_lands_and_the_previous_document_is_kept() {
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        let stored = svc.write(&admin(), 0, changed()).await.expect("written");
        assert_eq!(stored.settings, changed());
        assert_eq!(stored.revision, 1);

        let again = svc
            .write(&admin(), 1, PlatformSettings::default())
            .await
            .expect("written");
        assert_eq!(
            again.previous.as_ref(),
            Some(&changed()),
            "an overwrite must be recoverable in the moment"
        );
    }

    #[tokio::test]
    async fn an_administrator_can_add_a_rate_but_not_change_one() {
        let store = Arc::new(FakeStore::default());
        let svc =
            PlatformSettingsService::new(Arc::clone(&store) as Arc<dyn PlatformSettingsStore>);
        let mut priced = PlatformSettings::default();
        priced.pricing = priced.pricing.appended(rate(), "2026-08-23T01:00:00Z");
        let written = svc
            .write(&admin(), 0, priced.clone())
            .await
            .expect("a first card may be appended");

        let mut doctored = written.settings.clone();
        doctored.pricing.cards[0].rate.cpu_micros_per_cpu_hour += 1;
        let refused = svc.write(&admin(), written.revision, doctored).await;
        let Err(CoreError::PriceListNotAppendOnly(detail)) = refused else {
            panic!("a rewritten rate must be refused, got {refused:?}");
        };
        assert!(detail.contains(written.settings.pricing.cards[0].id.as_str()));
        assert!(detail.contains("rate"));

        let current = svc.read(&admin()).await.expect("the original still reads");
        assert_eq!(current.settings.pricing, priced.pricing);
        assert_eq!(current.revision, written.revision);
    }

    #[tokio::test]
    async fn a_deployment_that_configures_no_price_still_reads_and_still_serves() {
        let svc = PlatformSettingsService::unconfigured();
        let configured = svc.read(&admin()).await.expect("defaults are readable");
        let provisioning = svc.for_provisioning().await;

        assert!(configured.settings.pricing.cards.is_empty());
        assert!(provisioning.pricing.cards.is_empty());
    }

    #[tokio::test]
    async fn changing_the_briefing_does_not_disturb_the_price_list() {
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        let mut priced = PlatformSettings::default();
        priced.pricing = priced.pricing.appended(rate(), "2026-08-23T01:00:00Z");
        let first = svc
            .write(&admin(), 0, priced)
            .await
            .expect("the price is written");

        let mut prose_changed = first.settings.clone();
        prose_changed.briefing = changed().briefing;
        let second = svc
            .write(&admin(), first.revision, prose_changed)
            .await
            .expect("prose alone remains writable");

        assert_eq!(second.settings.pricing, first.settings.pricing);
    }

    #[tokio::test]
    async fn a_write_may_set_bounds_narrower_than_an_unnamed_session() {
        // The service deliberately has no numeric version of the deployment's
        // own reservation. Bounds govern only a size a caller states, so there
        // is no deployment default to compare this write against.
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        let mut settings = PlatformSettings::default();
        settings.size_bounds.max_memory_mib =
            std::num::NonZeroU32::new(512).expect("the test maximum is non-zero");

        let written = svc
            .write(&admin(), 0, settings.clone())
            .await
            .expect("narrow bounds remain writable");

        assert_eq!(written.settings, settings);
    }

    #[tokio::test]
    async fn a_second_writer_working_from_a_stale_read_is_told_rather_than_applied() {
        /*
         * Two administrators with the settings page open is the ordinary case
         * for a value that changes rarely and is opened by whoever is asking
         * about it. Last-write-wins would discard one of them silently, and
         * neither would ever know.
         */
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        let first = svc.read(&admin()).await.expect("readable");
        let second = svc.read(&admin()).await.expect("readable");

        svc.write(&admin(), first.revision, changed())
            .await
            .expect("the first writer wins");

        let refused = svc
            .write(&admin(), second.revision, PlatformSettings::default())
            .await;
        assert!(
            matches!(refused, Err(CoreError::SettingsConflict(_))),
            "got {refused:?}"
        );

        // And the first writer's work survived, which is the point.
        let now = svc.read(&admin()).await.expect("readable");
        assert_eq!(now.settings, changed());
    }

    #[tokio::test]
    async fn reset_restores_only_the_shipped_words_without_the_client_holding_them() {
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        let mut configured = changed();
        configured.size_bounds.max_memory_mib =
            std::num::NonZeroU32::new(4_096).expect("the test bound is non-zero");
        configured.live_sessions =
            Some(std::num::NonZeroU32::new(3).expect("the test limit is non-zero"));
        let written = svc
            .write(&admin(), 0, configured.clone())
            .await
            .expect("written");
        let back = svc
            .reset_briefing(&admin(), written.revision)
            .await
            .expect("reset");
        assert_eq!(back.settings.briefing, SessionBriefing::default());
        assert_eq!(back.settings.size_bounds, configured.size_bounds);
        assert_eq!(back.settings.live_sessions, configured.live_sessions);
    }

    #[tokio::test]
    async fn provisioning_never_fails_and_never_asks_who_is_calling() {
        /*
         * A session may be brought back by reconciliation with nobody on the
         * line, so this path takes no caller and can never be handed one to
         * check. And it must not fail: refusing to start a session because a
         * settings document could not be fetched is far worse than one
         * briefly reading the shipped words, which is what every session got
         * before any of this existed.
         */
        let broken = PlatformSettingsService::new(Arc::new(FakeStore {
            broken: true,
            ..Default::default()
        }));
        assert_eq!(
            broken.for_provisioning().await,
            PlatformSettings::default(),
            "an unreadable store must not stop a session starting"
        );
        // The administrator, by contrast, is told plainly.
        assert!(matches!(
            broken.read(&admin()).await,
            Err(CoreError::SettingsUnavailable(_))
        ));
    }

    #[tokio::test]
    async fn provisioning_reads_what_an_administrator_wrote() {
        // The mutation the test above would not catch: always returning the
        // defaults. Then nothing an administrator does ever reaches a session.
        let svc = PlatformSettingsService::new(Arc::new(FakeStore::default()));
        svc.write(&admin(), 0, changed()).await.expect("written");
        assert_eq!(svc.for_provisioning().await, changed());
    }

    #[tokio::test]
    async fn a_deployment_with_no_store_reads_but_refuses_to_pretend_it_saved() {
        // A settings page that says "saved" and changes nothing is worse than
        // one that says it cannot.
        let svc = PlatformSettingsService::unconfigured();
        assert!(svc.read(&admin()).await.is_ok());
        assert!(matches!(
            svc.write(&admin(), 0, changed()).await,
            Err(CoreError::SettingsUnavailable(_))
        ));
    }
}
