//! Opt-in native intake and durable inbox recovery owned by service shutdown.
use super::{
    inbox_dispatch::InboxDispatcher,
    runtime_source,
    source_worker::{self, Attempt, SourcePort, Timing},
    DiscordApi, DurableJournal, GatewayError,
};
use crate::{
    channels::resolver::ChannelResolver, http::readiness::Readiness, ingress::IngressState,
    records::RecordClient, NativeDiscordConfig,
};
use cmaeg_core::seal::MasterKeyRing;
use futures_util::future::BoxFuture;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::watch,
    task::{JoinHandle, JoinSet},
};

struct Production {
    routing: Arc<crate::ingress::routed::RoutedIngress>,
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    resolver: ChannelResolver,
    api: DiscordApi,
    activity: Arc<dyn crate::turns::observer::ActivityObserver>,
}
impl SourcePort for Production {
    fn revision<'a>(&'a self, record: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        Box::pin(async move {
            let current =
                runtime_source::resolve(&self.records, &self.keys, &self.resolver, record).await?;
            if !self.routing.native_eligible(&current.channel.definition) {
                return Err(GatewayError::InvalidConfiguration);
            }
            Ok(current.revision)
        })
    }
    fn connect<'a>(
        &'a self,
        record: &'a str,
        stop: watch::Receiver<bool>,
    ) -> BoxFuture<'a, Attempt> {
        Box::pin(async move {
            let result = match DurableJournal::acquire(
                self.records.clone(),
                self.keys.clone(),
                record,
                self.api.clone(),
                self.activity.clone(),
            )
            .await
            {
                Ok(journal) => journal.run_connection(stop).await,
                Err(error) => Err(error),
            };
            let cooldown =
                match runtime_source::resolve(&self.records, &self.keys, &self.resolver, record)
                    .await
                {
                    Ok(current) => current
                        .channel
                        .definition
                        .secrets
                        .discord_bot_token
                        .as_ref()
                        .map_or(Duration::ZERO, |token| self.api.retry_after(token)),
                    Err(_) => Duration::ZERO,
                };
            Attempt { result, cooldown }
        })
    }
}

impl super::source_discovery::DiscoveryPort for Production {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<crate::records::RecordHead>, GatewayError>> {
        Box::pin(async move {
            self.records
                .list(None, after, super::source_discovery::PAGE_SIZE)
                .await
                .map_err(|_| GatewayError::Unavailable)
        })
    }
    fn eligible<'a>(
        &'a self,
        record: &'a str,
        owner: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), GatewayError>> {
        Box::pin(async move {
            let current =
                runtime_source::resolve(&self.records, &self.keys, &self.resolver, record).await?;
            if owner.is_some_and(|owner| owner != current.channel.definition.owner.subject)
                || !self.routing.native_eligible(&current.channel.definition)
            {
                return Err(GatewayError::InvalidConfiguration);
            }
            Ok(())
        })
    }
}

pub(crate) struct Worker {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<bool>>,
}
impl Worker {
    pub fn start(
        config: &NativeDiscordConfig,
        selection: super::selection::Selection,
        thread_metadata: bool,
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        ingress: IngressState,
        readiness: Readiness,
    ) -> Result<Option<Self>, GatewayError> {
        // Authoring a native source does not activate sockets. The explicit
        // runtime gate plus explicit canaries or bounded discovery must be present.
        if !config.enabled || (config.records.is_empty() && !config.discovery_enabled) {
            return Ok(None);
        }
        let selected = config.records.clone();
        let Some(routing) = ingress.routed.as_ref() else {
            return Err(GatewayError::InvalidConfiguration);
        };
        if selected.iter().any(|record| {
            cmaeg_core::ids::RecordId::new(record).map_or(true, |record| !routing.selected(&record))
        }) {
            return Err(GatewayError::InvalidConfiguration);
        }
        let port = Arc::new(Production {
            routing: routing.clone(),
            records: records.clone(),
            keys: keys.clone(),
            resolver: ingress.resolver.clone(),
            api: DiscordApi::new()?
                .with_thread_metadata(thread_metadata)
                .with_components(ingress.clone()),
            activity: ingress.activity.clone(),
        });
        let dispatcher = InboxDispatcher::with_selection(records, keys, ingress, selection.clone());
        let discovery_enabled = config.discovery_enabled;
        let (stop, stopped) = watch::channel(false);
        let signal = stop.clone();
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            if discovery_enabled {
                tasks.spawn(super::source_discovery::run(
                    port.clone(),
                    selection,
                    selected,
                    stopped.clone(),
                ));
            } else {
                for record in selected {
                    tasks.spawn(source_worker::run(
                        port.clone(),
                        record,
                        stopped.clone(),
                        Timing::default(),
                    ));
                }
            }
            tasks.spawn(inbox_loop(dispatcher, stopped.clone()));
            let mut stopping = stopped.clone();
            let healthy = tokio::select! {
                biased;
                _ = stopping.changed() => true,
                _ = tasks.join_next() => {
                    readiness.routed_recovery_failed();
                    tracing::error!("native Discord worker stopped unexpectedly");
                    false
                }
            };
            let _ = signal.send(true);
            while let Some(result) = tasks.join_next().await {
                if result.is_err() {
                    return false;
                }
            }
            healthy
        });
        Ok(Some(Self {
            stop,
            task: Some(task),
        }))
    }
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }
    pub async fn drain(&mut self, deadline: Instant) -> bool {
        self.stop();
        let Some(mut task) = self.task.take() else {
            return true;
        };
        match tokio::time::timeout_at(deadline.into(), &mut task).await {
            Ok(Ok(healthy)) => healthy,
            Ok(Err(_)) => false,
            Err(_) => {
                task.abort();
                let _ = task.await;
                false
            }
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
async fn inbox_loop(dispatcher: InboxDispatcher, mut stop: watch::Receiver<bool>) {
    let mut cursor = None;
    loop {
        if *stop.borrow() {
            return;
        }
        let page = tokio::select! {
            _ = stop.changed() => return,
            result = dispatcher.page(cursor.as_deref(), time::OffsetDateTime::now_utc().unix_timestamp()) => result,
        };
        let delay = match page {
            Ok(page) => {
                if page.invalid > 0 {
                    tracing::warn!(count = page.invalid, "native Discord inbox records refused");
                }
                cursor = page.next_cursor;
                if cursor.is_some() {
                    Duration::from_millis(250)
                } else {
                    Duration::from_secs(5)
                }
            }
            Err(_) => Duration::from_secs(5),
        };
        if source_worker::stopped(&mut stop, delay).await {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    #[tokio::test]
    async fn disabled_or_empty_native_runtime_starts_no_worker() {
        let f = super::super::dispatch_fixture::Setup::new(true).await;
        for config in [
            NativeDiscordConfig::default(),
            NativeDiscordConfig {
                enabled: false,
                discovery_enabled: false,
                records: BTreeSet::from([f.fixture.record.clone()]),
            },
            NativeDiscordConfig {
                enabled: true,
                discovery_enabled: false,
                records: BTreeSet::new(),
            },
        ] {
            assert!(Worker::start(
                &config,
                super::super::selection::Selection::new(&config),
                false,
                RecordClient::new("http://127.0.0.1:1", "no-network-fixture").unwrap(),
                f.fixture.store.keys.clone(),
                f.state.clone(),
                Readiness::initialized()
            )
            .unwrap()
            .is_none());
        }
    }
    #[tokio::test]
    async fn current_native_eligibility_checks_owner_routing_and_credentials() {
        use super::super::source_discovery::DiscoveryPort;
        let f = super::super::dispatch_fixture::Setup::new(true).await;
        let port = Production {
            records: f.fixture.store.channels.clone(),
            keys: f.fixture.store.keys.clone(),
            resolver: f.state.resolver.clone(),
            api: f.fixture.api.clone(),
            activity: f.state.activity.clone(),
            routing: f.state.routed.clone().unwrap(),
        };
        assert!(port
            .eligible(&f.fixture.record, Some(&f.channel.owner.subject))
            .await
            .is_ok());
        assert_eq!(
            port.eligible(&f.fixture.record, Some("wrong-owner")).await,
            Err(GatewayError::InvalidConfiguration)
        );
        let mut changed = f.channel.clone();
        changed.routing = None;
        f.fixture.replace(&changed);
        assert_eq!(
            port.eligible(&f.fixture.record, None).await,
            Err(GatewayError::InvalidConfiguration)
        );
        changed = f.channel.clone();
        changed.secrets.discord_bot_token = None;
        f.fixture.replace(&changed);
        assert_eq!(
            port.eligible(&f.fixture.record, None).await,
            Err(GatewayError::Authority)
        );
    }
}
