//! Coalesce only overlapping empty-discovery opportunities, never later ticks.
use futures_util::TryStreamExt;
use mongodb::{
    bson::{doc, Document},
    options::ReadConcern,
    Collection,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{oneshot, Semaphore};

const MAX_BATCH: usize = 16;
const PROBE_DEADLINE: Duration = Duration::from_millis(25);

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Commands,
    Controls,
    Agents,
    Sandboxes,
    Workspaces,
    Bindings,
    Conversations,
    Access,
    Launches,
}
impl Kind {
    fn label(self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Controls => "controls",
            Self::Agents => "agents",
            Self::Sandboxes => "sandboxes",
            Self::Workspaces => "workspaces",
            Self::Bindings => "bindings",
            Self::Conversations => "binding_conversations",
            Self::Access => "access",
            Self::Launches => "launches",
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct Policy(Option<Arc<Batches>>);
impl Policy {
    pub(super) fn new(enabled: bool) -> Self {
        Self(enabled.then(|| Arc::new(Batches::default())))
    }

    pub(super) async fn empty(
        &self,
        kind: Kind,
        collection: &Collection<Document>,
        pending: Document,
    ) -> bool {
        let Some(batches) = &self.0 else { return false };
        if !batches.eligible[kind as usize].load(Ordering::Relaxed) {
            return false;
        }
        if batches
            .positive_until
            .lock()
            .unwrap_or_else(|e| e.into_inner())[kind as usize]
            .is_some_and(|until| tokio::time::Instant::now() < until)
        {
            return false;
        }
        let (sender, receiver) = oneshot::channel();
        let start = {
            let mut queue = batches.queue.lock().unwrap_or_else(|e| e.into_inner());
            if queue.len() >= MAX_BATCH {
                return false;
            }
            let start = queue.is_empty();
            queue.push(Request {
                kind,
                collection: collection.clone(),
                pending,
                sender,
            });
            start
        };
        if start {
            let batches = batches.clone();
            tokio::spawn(async move {
                batches.run().await;
            });
        }
        // Cancellation leaves at most one bounded queued sender. No detached
        // scanner is allowed to hold callers past the original due cadence.
        let empty = tokio::time::timeout(PROBE_DEADLINE * 2, receiver)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(false);
        #[cfg(test)]
        if empty {
            batches.skipped.fetch_add(1, Ordering::Relaxed);
        }
        metrics::counter!("cma_empty_discovery_total", "worker" => kind.label(),
            "outcome" => if empty { "coalesced_empty" } else { "ordinary" })
        .increment(1);
        empty
    }

    #[cfg(test)]
    pub(super) fn counts(&self) -> (u64, u64) {
        self.0.as_ref().map_or((0, 0), |b| {
            (
                b.batches.load(Ordering::Relaxed),
                b.skipped.load(Ordering::Relaxed),
            )
        })
    }

    pub(super) fn observed(&self, kind: Kind, empty: bool) {
        if let Some(batches) = &self.0 {
            batches.eligible[kind as usize].store(empty, Ordering::Relaxed);
        }
    }
}

struct Request {
    kind: Kind,
    collection: Collection<Document>,
    pending: Document,
    sender: oneshot::Sender<bool>,
}
struct Batches {
    queue: Mutex<Vec<Request>>,
    slots: Semaphore,
    eligible: [AtomicBool; 9],
    positive_until: Mutex<[Option<tokio::time::Instant>; 9]>,
    #[cfg(test)]
    batches: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    skipped: std::sync::atomic::AtomicU64,
}
impl Default for Batches {
    fn default() -> Self {
        Self {
            queue: Mutex::new(Vec::new()),
            slots: Semaphore::new(2),
            positive_until: Mutex::new([None; 9]),
            #[cfg(test)]
            batches: Default::default(),
            #[cfg(test)]
            skipped: Default::default(),
            eligible: std::array::from_fn(|_| AtomicBool::new(true)),
        }
    }
}
impl Batches {
    async fn run(&self) {
        // Collect already runnable requests without a batching timer. After
        // take(), arrivals form a NEW batch even while this query is in flight.
        tokio::task::yield_now().await;
        let mut requests: Vec<_> =
            std::mem::take(&mut *self.queue.lock().unwrap_or_else(|e| e.into_inner()))
                .into_iter()
                .filter(|r| !r.sender.is_closed())
                .collect();
        // The command reader requires this index, even for an empty result.
        // Put it first so the aggregate can preserve that mandatory hint.
        requests.sort_by_key(|r| !matches!(r.kind, Kind::Commands));
        let permit = self.slots.try_acquire();
        if requests.len() < 2 || permit.is_err() {
            for request in requests {
                let _ = request.sender.send(false);
            }
            return;
        }
        #[cfg(test)]
        self.batches.fetch_add(1, Ordering::Relaxed);
        let started = std::time::Instant::now();
        // Callers have their own finite wait. Keep this slot until the driver
        // actually completes, including after a caller times out or cancels;
        // dropping an IO future would not prove the server stopped the probe.
        // Two slow probes therefore disable extra probing instead of spawning
        // unbounded unobserved operations. Ordinary discovery remains usable.
        let present = probe(&requests).await.ok();
        metrics::histogram!("cma_empty_discovery_probe_seconds")
            .record(started.elapsed().as_secs_f64());
        metrics::histogram!("cma_empty_discovery_batch_requests").record(requests.len() as f64);
        metrics::counter!("cma_empty_discovery_batches_total", "outcome" => if present.is_some() { "success" } else { "fallback" }).increment(1);
        for (index, request) in requests.into_iter().enumerate() {
            let empty = present.as_ref().is_some_and(|p| !p[index]);
            if !empty {
                self.eligible[request.kind as usize].store(false, Ordering::Relaxed);
                // This suppresses only extra probes. Ordinary discovery keeps
                // its existing cadence, including future due work and leases.
                self.positive_until
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())[request.kind as usize] =
                    Some(tokio::time::Instant::now() + Duration::from_secs(30));
            }
            // A dropped receiver means its ordinary scheduler was cancelled.
            let _ = request.sender.send(empty);
        }
    }
}

fn branch(request: &Request, index: usize) -> Vec<Document> {
    vec![
        doc! {"$match":request.pending.clone()},
        doc! {"$limit":1_i64},
        doc! {"$project":{"_id":0,"probe":{"$literal":index as i64}}},
    ]
}
async fn probe(requests: &[Request]) -> Result<Vec<bool>, ()> {
    let first = &requests[0];
    let names: std::collections::BTreeSet<_> =
        requests.iter().map(|r| r.collection.name()).collect();
    let database = first
        .collection
        .client()
        .database(&first.collection.namespace().db);
    let actual = database
        .list_collection_names()
        .filter(doc! {"name":{"$in":names.iter().copied().collect::<Vec<_>>()},"type":"collection"})
        .await
        .map_err(|_| ())?;
    if actual.len() != names.len() || actual.iter().any(|name| !names.contains(name.as_str())) {
        return Err(());
    }
    let mut pipeline = branch(first, 0);
    for (index, request) in requests.iter().enumerate().skip(1) {
        if request.collection.namespace().db != first.collection.namespace().db {
            return Err(());
        }
        pipeline.push(
            doc! {"$unionWith":{"coll":request.collection.name(),"pipeline":branch(request,index)}},
        );
    }
    // Every result belongs to one fresh majority-committed snapshot; access and
    // launch effects still establish their own ordinary transaction snapshot.
    let mut query = first
        .collection
        .aggregate(pipeline)
        .read_concern(ReadConcern::snapshot())
        .allow_disk_use(false)
        .max_time(PROBE_DEADLINE);
    if matches!(first.kind, Kind::Commands) {
        query = query.hint(mongodb::options::Hint::Name(
            "agent_command_work_scan".into(),
        ));
    }
    let mut cursor = query.await.map_err(|_| ())?;
    let mut present = vec![false; requests.len()];
    while let Some(row) = cursor.try_next().await.map_err(|_| ())? {
        let index = usize::try_from(row.get_i64("probe").map_err(|_| ())?).map_err(|_| ())?;
        let value = present.get_mut(index).ok_or(())?;
        if *value || row.len() != 1 {
            return Err(());
        }
        *value = true;
    }
    Ok(present)
}
