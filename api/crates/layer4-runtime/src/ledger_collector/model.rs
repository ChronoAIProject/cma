//! Test model only. This exercises coordinator ordering, not V1/V2 storage.
use super::*;
#[path = "model_archive.rs"]
mod archive;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Call {
    Evidence,
    Retain,
    Reference,
    Source,
    Cold,
    Select,
    Dispatch,
    Record,
    Settle,
}

#[derive(Clone)]
pub struct Model {
    pub state: Arc<Mutex<State>>,
    pub holds: Arc<AtomicUsize>,
    pub acquires: Arc<AtomicUsize>,
    pub loads: Arc<AtomicUsize>,
    pub resume: Arc<tokio::sync::Notify>,
}

pub struct State {
    pub checkpoint: Checkpoint,
    pub required_hold: Option<Arc<AtomicUsize>>,
    pub source_revision: u64,
    pub groups: BTreeMap<Digest, Decision>,
    pub evidence: BTreeMap<Operation, Intent>,
    pub cold: BTreeMap<Operation, Receipt>,
    pub references: BTreeMap<Operation, CollectionReference>,
    pub retained_pages: BTreeMap<Operation, Page>,
    pub retained_originals: BTreeMap<Operation, [Vec<u8>; 3]>,
    pub archive_pending: Option<Operation>,
    pub source_committed: Option<Receipt>,
    pub retentions: usize,
    pub staged: Option<Intent>,
    pub next_operation: u128,
    pub mutations: usize,
    pub enumerations: usize,
    pub selected: Vec<Operation>,
    pub fail_after: Option<Call>,
    pub unavailable: Option<Refusal>,
    pub corrupt_receipt: bool,
    pub wrong_authority: bool,
    pub suspend_dispatch: bool,
    pub in_flight: Option<Arc<ModelHold>>,
    pub retired_operations: u64,
    pub retired_authority: u64,
}

pub struct ModelHold {
    binding: Binding,
    revision: u64,
    count: Arc<AtomicUsize>,
}
impl Held for ModelHold {
    fn binding(&self) -> Binding {
        self.binding
    }
    fn source_revision(&self) -> u64 {
        self.revision
    }
}
impl Drop for ModelHold {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn digest(value: u64) -> Digest {
    let mut bytes = [0; 32];
    bytes[24..].copy_from_slice(&value.to_be_bytes());
    bytes
}

impl Model {
    pub fn new(format: SourceFormat, groups: &[(u64, Decision)]) -> Self {
        let binding = Binding {
            sha256: digest(9000),
            archive_id: 42u128.to_be_bytes(),
            epoch: 1,
            format,
        };
        let checkpoint = Checkpoint {
            revision: 1,
            fence: 1,
            binding,
            source_revision: 1,
            after_group: None,
            cursor: None,
            page: None,
            staged: None,
            pending: None,
            complete: false,
        };
        Self {
            holds: Arc::default(),
            acquires: Arc::default(),
            loads: Arc::default(),
            resume: Arc::default(),
            state: Arc::new(Mutex::new(State {
                checkpoint,
                required_hold: None,
                source_revision: 1,
                groups: groups.iter().map(|(k, d)| (digest(*k), *d)).collect(),
                evidence: BTreeMap::new(),
                cold: BTreeMap::new(),
                references: BTreeMap::new(),
                retained_pages: BTreeMap::new(),
                retained_originals: BTreeMap::new(),
                archive_pending: None,
                source_committed: None,
                retentions: 0,
                staged: None,
                next_operation: 1,
                mutations: 0,
                enumerations: 0,
                selected: vec![],
                fail_after: None,
                unavailable: None,
                corrupt_receipt: false,
                wrong_authority: false,
                suspend_dispatch: false,
                in_flight: None,
                retired_operations: 0,
                retired_authority: 0,
            })),
        }
    }

    pub fn reopened(&self) -> Self {
        self.clone()
    }

    pub fn add_cycle(&self, key: u64) {
        let mut s = self.state.lock().unwrap();
        assert!(!s.checkpoint.has_work() && s.checkpoint.page.is_none());
        s.groups.insert(digest(key), Decision::SettledDeleted);
        s.source_revision += 1;
    }
}

fn cas(s: &State, expected: &Checkpoint) -> Result<()> {
    if let Some(count) = &s.required_hold {
        assert_eq!(
            count.load(Ordering::SeqCst),
            1,
            "gate lost across progress CAS"
        );
    }
    if s.checkpoint == *expected {
        Ok(())
    } else {
        Err(Refusal::Conflict)
    }
}
fn fail_after(s: &mut State, call: Call) -> Result<()> {
    if s.fail_after == Some(call) {
        s.fail_after = None;
        Err(Refusal::UnknownOutcome)
    } else {
        Ok(())
    }
}

impl CollectorPort for Model {
    type Hold = ModelHold;
    async fn load(&self) -> Result<Checkpoint> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        Ok(self.state.lock().unwrap().checkpoint.clone())
    }
    async fn pending_collection(&self, binding: &Binding) -> Result<Option<CollectionReference>> {
        self.archive_pending(binding)
    }
    async fn recover_reference(
        &self,
        expected: &Checkpoint,
        reference: CollectionReference,
    ) -> Result<()> {
        self.recover_retained(expected, reference)
    }
    async fn acquire(&self, expected: &Checkpoint) -> Result<Arc<ModelHold>> {
        let s = self.state.lock().unwrap();
        if s.checkpoint != *expected {
            return Err(Refusal::Conflict);
        }
        if s.in_flight.is_some() {
            return Err(Refusal::Conflict);
        }
        let mut binding = s.checkpoint.binding;
        if s.wrong_authority {
            binding.epoch += 1;
        }
        self.holds.fetch_add(1, Ordering::SeqCst);
        self.acquires.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(ModelHold {
            binding,
            revision: s.source_revision,
            count: self.holds.clone(),
        }))
    }
    async fn enumerate(&self, _: Arc<ModelHold>, expected: &Checkpoint) -> Result<Page> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        if s.source_revision != expected.source_revision {
            return Err(Refusal::StaleCursor);
        }
        s.enumerations += 1;
        let groups: Vec<_> = s
            .groups
            .keys()
            .copied()
            .filter(|k| expected.after_group.is_none_or(|after| *k > after))
            .take(65)
            .collect();
        let complete = groups.len() <= 64;
        Ok(Page {
            source_revision: s.source_revision,
            sha256: digest(s.source_revision),
            cursor: expected.cursor.clone(),
            next_cursor: (!complete).then(|| format!("page-{}", s.enumerations)),
            complete,
            groups: groups.into_iter().take(64).collect(),
            encoded_bytes: 2048,
        })
    }
    async fn pin_page(&self, expected: &Checkpoint, page: Page) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        s.checkpoint.page = Some(page);
        s.checkpoint.revision += 1;
        Ok(())
    }
    async fn observe_and_retain(
        &self,
        _: Arc<ModelHold>,
        expected: &Checkpoint,
        group: Digest,
    ) -> Result<Staged> {
        let (intent, originals) = {
            let mut s = self.state.lock().unwrap();
            cas(&s, expected)?;
            if expected.has_work() {
                return Err(Refusal::Conflict);
            }
            let operation = s.next_operation.to_be_bytes();
            s.next_operation += 1;
            archive::prepared(
                expected,
                group,
                operation,
                *s.groups.get(&group).ok_or(Refusal::InvalidEvidence)?,
            )
        };
        let reference = if intent.decision == Decision::SettledDeleted {
            Some(self.archive_retain(
                &intent,
                originals,
                expected.page.clone().ok_or(Refusal::InvalidEvidence)?,
            )?)
        } else {
            None
        };
        fail_after(&mut self.state.lock().unwrap(), Call::Evidence)?;
        Ok(Staged { intent, reference })
    }
    async fn verify_prepared(&self, intent: &Intent) -> Result<()> {
        let s = self.state.lock().unwrap();
        if s.staged.as_ref() != Some(intent) {
            return Err(Refusal::InvalidEvidence);
        }
        if intent.decision == Decision::SettledDeleted {
            archive::read(&s, intent, &intent.reference())?;
        }
        Ok(())
    }
    async fn record_reference(&self, expected: &Checkpoint, staged: Staged) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        if !staged.valid() || expected.has_work() {
            return Err(Refusal::InvalidEvidence);
        }
        if let Some(reference) = &staged.reference {
            archive::read(&s, &staged.intent, reference)?;
        }
        s.staged = Some(staged.intent.clone());
        s.checkpoint.staged = Some(staged);
        s.checkpoint.revision += 1;
        fail_after(&mut s, Call::Reference)
    }
    async fn select(&self, expected: &Checkpoint) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        let staged = s
            .checkpoint
            .staged
            .as_ref()
            .ok_or(Refusal::InvalidEvidence)?;
        let pending = Pending {
            intent: staged.intent.clone(),
            reference: staged.reference.clone(),
            completion: None,
        };
        if !pending.valid() {
            return Err(Refusal::InvalidEvidence);
        }
        s.selected.push(pending.intent.operation);
        s.checkpoint.staged = None;
        s.checkpoint.pending = Some(pending);
        s.checkpoint.revision += 1;
        fail_after(&mut s, Call::Select)
    }
    async fn read_collection(
        &self,
        intent: &Intent,
        reference: &CollectionReference,
    ) -> Result<()> {
        archive::read(&self.state.lock().unwrap(), intent, reference)
    }
    async fn collection_completion(
        &self,
        intent: &Intent,
        reference: &CollectionReference,
    ) -> Result<Option<Receipt>> {
        let s = self.state.lock().unwrap();
        archive::read(&s, intent, reference)?;
        let mut receipt = s.cold.get(&intent.operation).cloned();
        if s.corrupt_receipt {
            if let Some(r) = &mut receipt {
                r.plan_sha256 = digest(9999);
            }
        }
        Ok(receipt)
    }
    async fn complete_collection_held(
        &self,
        hold: Arc<ModelHold>,
        intent: &Intent,
        reference: &CollectionReference,
    ) -> Result<Receipt> {
        let receipt = self.archive_complete(intent, reference)?;
        let suspend = {
            let mut s = self.state.lock().unwrap();
            fail_after(&mut s, Call::Dispatch)?;
            if s.suspend_dispatch {
                s.in_flight = Some(hold);
            }
            s.suspend_dispatch
        };
        if suspend {
            self.resume.notified().await;
        }
        Ok(receipt)
    }
    async fn record_receipt(&self, expected: &Checkpoint, receipt: Receipt) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        let pending = s.checkpoint.pending.as_mut().unwrap();
        if !receipt.matches(&pending.intent) {
            return Err(Refusal::InvalidEvidence);
        }
        pending.completion = Some(receipt);
        s.checkpoint.revision += 1;
        fail_after(&mut s, Call::Record)
    }
    async fn settle(&self, expected: &Checkpoint) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        let pending = s.checkpoint.pending.take().unwrap();
        s.checkpoint.after_group = Some(pending.intent.group);
        if pending.intent.decision == Decision::SettledDeleted {
            let receipt = pending.completion.as_ref().unwrap();
            assert_eq!(s.cold.get(&pending.intent.operation), Some(receipt));
            s.checkpoint.source_revision = receipt.after_revision;
            s.checkpoint.page = None;
            s.checkpoint.cursor = None;
        } else {
            s.checkpoint
                .page
                .as_mut()
                .unwrap()
                .groups
                .retain(|k| *k != pending.intent.group);
        }
        s.staged = None;
        s.checkpoint.revision += 1;
        fail_after(&mut s, Call::Settle)
    }
    async fn advance_page(&self, expected: &Checkpoint) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        let page = s.checkpoint.page.take().unwrap();
        assert!(page.groups.is_empty());
        s.checkpoint.cursor = page.next_cursor;
        s.checkpoint.complete = page.complete;
        s.checkpoint.revision += 1;
        Ok(())
    }
    async fn refresh_revision(&self, expected: &Checkpoint, revision: u64) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        assert!(!s.checkpoint.has_work());
        if s.checkpoint.complete {
            s.checkpoint.after_group = None;
        }
        s.checkpoint.complete = false;
        s.checkpoint.source_revision = revision;
        s.checkpoint.page = None;
        s.checkpoint.cursor = None;
        s.checkpoint.revision += 1;
        Ok(())
    }
}
