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
    records: RecordClient,
    keys: Arc<MasterKeyRing>,
    resolver: ChannelResolver,
    api: DiscordApi,
}
impl SourcePort for Production {
    fn revision<'a>(&'a self, record: &'a str) -> BoxFuture<'a, Result<String, GatewayError>> {
        Box::pin(async move {
            let current =
                runtime_source::resolve(&self.records, &self.keys, &self.resolver, record).await?;
            if current.channel.definition.routing.is_none() {
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

pub(crate) struct Worker {
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<bool>>,
}
impl Worker {
    pub fn start(
        config: &NativeDiscordConfig,
        records: RecordClient,
        keys: Arc<MasterKeyRing>,
        ingress: IngressState,
        readiness: Readiness,
    ) -> Result<Option<Self>, GatewayError> {
        // Authoring a native source does not activate sockets. The explicit
        // runtime gate and selected records must both be present.
        if !config.enabled || config.records.is_empty() {
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
            records: records.clone(),
            keys: keys.clone(),
            resolver: ingress.resolver.clone(),
            api: DiscordApi::new()?,
        });
        let dispatcher = InboxDispatcher::new(records, keys, ingress, selected.clone());
        let (stop, stopped) = watch::channel(false);
        let signal = stop.clone();
        let task = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            for record in selected {
                tasks.spawn(source_worker::run(
                    port.clone(),
                    record,
                    stopped.clone(),
                    Timing::default(),
                ));
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
                records: BTreeSet::from([f.fixture.record.clone()]),
            },
            NativeDiscordConfig {
                enabled: true,
                records: BTreeSet::new(),
            },
        ] {
            assert!(Worker::start(
                &config,
                RecordClient::new("http://127.0.0.1:1", "no-network-fixture").unwrap(),
                f.fixture.store.keys.clone(),
                f.state.clone(),
                Readiness::initialized()
            )
            .unwrap()
            .is_none());
        }
    }
}
