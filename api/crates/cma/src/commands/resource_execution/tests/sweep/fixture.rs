use super::*;
use std::collections::{BTreeSet, HashSet};
use tokio::sync::Semaphore;

pub(super) fn operation_id(n: u32) -> wire::Digest {
    wire::Digest::new(&format!("{n:064x}")).unwrap()
}

pub(super) struct State {
    pub pending: BTreeSet<wire::Digest>,
    pub scans: Vec<Option<wire::Digest>>,
    pub starts: Vec<wire::Digest>,
    pub active: HashSet<wire::Digest>,
    pub peak: usize,
    pub cancelled: Vec<wire::Digest>,
    pub held: HashSet<wire::Digest>,
    pub errors: HashSet<wire::Digest>,
    pub panics: HashSet<wire::Digest>,
    pub fail_scan: Option<usize>,
    pub stall_scan: bool,
}

pub(super) struct Work {
    pub state: Mutex<State>,
    pub permits: Semaphore,
}

impl Work {
    pub fn new(count: u32) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State {
                pending: (1..=count).map(operation_id).collect(),
                scans: Vec::new(),
                starts: Vec::new(),
                active: HashSet::new(),
                peak: 0,
                cancelled: Vec::new(),
                held: HashSet::new(),
                errors: HashSet::new(),
                panics: HashSet::new(),
                fail_scan: None,
                stall_scan: false,
            }),
            permits: Semaphore::new(0),
        })
    }
    pub fn release(&self) {
        self.state.lock().unwrap().held.clear();
        self.permits.add_permits(4);
    }
}

struct Running<'a> {
    work: &'a Work,
    id: wire::Digest,
    finished: bool,
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        let mut state = self.work.state.lock().unwrap();
        assert!(state.active.remove(&self.id));
        if !self.finished {
            state.cancelled.push(self.id.clone());
        }
    }
}

#[async_trait::async_trait]
impl SweepWork for Work {
    async fn discover(&self, after: Option<&wire::Digest>) -> ExecutionResult<PendingPage> {
        let (stall, fail) = {
            let mut state = self.state.lock().unwrap();
            state.scans.push(after.cloned());
            (state.stall_scan, state.fail_scan == Some(state.scans.len()))
        };
        if stall {
            std::future::pending::<()>().await;
        }
        if fail {
            return Err(ExecutionError::Unavailable);
        }
        let state = self.state.lock().unwrap();
        let operations: Vec<_> = state
            .pending
            .iter()
            .filter(|id| after.is_none_or(|after| *id > after))
            .take(MAX_PAGE_MEMBERS as usize)
            .cloned()
            .collect();
        let next = (operations.len() == MAX_PAGE_MEMBERS as usize)
            .then(|| operations.last().unwrap().clone());
        Ok(PendingPage { operations, next })
    }

    async fn reconcile(&self, id: &wire::Digest) -> ExecutionResult<Progress> {
        let (held, fail, panic) = {
            let mut state = self.state.lock().unwrap();
            assert!(state.active.insert(id.clone()), "duplicate local child");
            state.starts.push(id.clone());
            state.peak = state.peak.max(state.active.len());
            (
                state.held.contains(id),
                state.errors.contains(id),
                state.panics.remove(id),
            )
        };
        let mut running = Running {
            work: self,
            id: id.clone(),
            finished: false,
        };
        assert!(!panic, "synthetic reconciliation panic");
        if held {
            self.permits.acquire().await.unwrap().forget();
        }
        running.finished = true;
        if fail {
            return Err(ExecutionError::Unknown);
        }
        self.state.lock().unwrap().pending.remove(id);
        Ok(Progress::Effective)
    }
}
