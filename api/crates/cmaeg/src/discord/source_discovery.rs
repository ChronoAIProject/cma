//! Current-index discovery. Partial scans never establish absence or revoke workers.
use super::{
    selection::{Selection, MAX_RECORDS},
    source_worker::{self, SourcePort, Timing},
    GatewayError,
};
use crate::records::RecordHead;
use futures_util::future::BoxFuture;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};
use tokio::{sync::watch, task::JoinSet};

pub(super) const PAGE_SIZE: usize = 32;
pub(super) trait DiscoveryPort: SourcePort {
    fn page<'a>(
        &'a self,
        after: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Vec<RecordHead>, GatewayError>>;
    /// Exact sealed current authority; an index head alone cannot authorize intake.
    fn eligible<'a>(
        &'a self,
        record: &'a str,
        owner: Option<&'a str>,
    ) -> BoxFuture<'a, Result<(), GatewayError>>;
}
#[derive(Default)]
pub(super) struct Scanner {
    cursor: Option<String>,
}
impl Scanner {
    pub async fn step<P: DiscoveryPort>(
        &mut self,
        port: &P,
        selection: &Selection,
        pinned: &BTreeSet<String>,
    ) -> Result<(), GatewayError> {
        // Only authoritative exact-record results may remove a discovered worker.
        // Storage/network failures retain selection; per-effect custody still fails closed.
        for record in selection.records().difference(pinned) {
            if matches!(
                port.eligible(record, None).await,
                Err(GatewayError::Authority
                    | GatewayError::InvalidConfiguration
                    | GatewayError::MissingMessageContentIntent)
            ) {
                selection.remove(record);
            }
        }
        let page = port.page(self.cursor.as_deref()).await?;
        if page.len() > PAGE_SIZE
            || page
                .windows(2)
                .any(|pair| pair[0].record_id >= pair[1].record_id)
            || page.first().is_some_and(|head| {
                self.cursor
                    .as_ref()
                    .is_some_and(|cursor| head.record_id <= *cursor)
            })
        {
            return Err(GatewayError::Protocol);
        }
        let mut capacity_refused = 0;
        for head in &page {
            if selection.contains(&head.record_id)
                || head.owner.is_empty()
                || !uuid::Uuid::parse_str(&head.record_id)
                    .is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == head.record_id)
            {
                continue;
            }
            if port
                .eligible(&head.record_id, Some(&head.owner))
                .await
                .is_ok()
                && !selection.insert(head.record_id.clone())
            {
                capacity_refused += 1;
            }
        }
        if capacity_refused > 0 {
            tracing::warn!(
                count = capacity_refused,
                capacity = MAX_RECORDS,
                "native Discord activation capacity exceeded"
            );
        }
        self.cursor = if page.len() == PAGE_SIZE {
            page.last().map(|head| head.record_id.clone())
        } else {
            None
        };
        Ok(())
    }
}

pub(super) async fn run<P: DiscoveryPort + 'static>(
    port: Arc<P>,
    selection: Selection,
    pinned: BTreeSet<String>,
    mut stop: watch::Receiver<bool>,
) {
    if *stop.borrow() {
        return;
    }
    let scan = scan_loop(port.clone(), selection.clone(), pinned, stop.clone());
    tokio::pin!(scan);
    let mut retiring = BTreeSet::new();
    let mut workers = BTreeMap::<String, watch::Sender<bool>>::new();
    let mut tasks = JoinSet::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            _ = stop.changed() => break,
            _ = &mut scan => break,
            result = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Some(Ok(record)) if retiring.remove(&record) => {},
                    _ => { tracing::error!("native Discord source supervisor stopped unexpectedly"); break; }
                }
            }
            _ = tick.tick() => {
                let current = selection.records();
                workers.retain(|record, signal| {
                    if current.contains(record) { true } else { let _ = signal.send(true); retiring.insert(record.clone()); false }
                });
                for record in current {
                    if tasks.len() >= MAX_RECORDS { break; }
                    if workers.contains_key(&record) || retiring.contains(&record) { continue; }
                    let (signal, stopped) = watch::channel(false);
                    workers.insert(record.clone(), signal);
                    let port = port.clone();
                    tasks.spawn(async move {
                        source_worker::run(port, record.clone(), stopped, Timing::default()).await;
                        record
                    });
                }
            }
        }
        if *stop.borrow() {
            break;
        }
    }
    for signal in workers.values() {
        let _ = signal.send(true);
    }
    while tasks.join_next().await.is_some() {}
}

async fn scan_loop<P: DiscoveryPort>(
    port: Arc<P>,
    selection: Selection,
    pinned: BTreeSet<String>,
    mut stop: watch::Receiver<bool>,
) {
    let mut scanner = Scanner::default();
    loop {
        let result = tokio::select! {
            _ = stop.changed() => return,
            result = scanner.step(port.as_ref(), &selection, &pinned) => result,
        };
        if result.is_err() {
            tracing::warn!(
                "native Discord channel discovery incomplete; retaining current selection"
            );
        }
        if source_worker::stopped(&mut stop, Duration::from_secs(5)).await {
            return;
        }
    }
}

#[cfg(test)]
#[path = "source_discovery_tests.rs"]
mod tests;
