//! Test model only. This exercises coordinator ordering, not V1/V2 storage.
use super::*;
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
    Select,
    Dispatch,
    Record,
    Settle,
}

#[derive(Clone)]
pub struct Model {
    pub state: Arc<Mutex<State>>,
    pub holds: Arc<AtomicUsize>,
}

pub struct State {
    pub checkpoint: Checkpoint,
    pub source_revision: u64,
    pub groups: BTreeMap<Digest, Decision>,
    pub evidence: BTreeMap<Operation, Intent>,
    pub cold: BTreeMap<Operation, Vec<Receipt>>,
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
            pending: None,
            complete: false,
        };
        Self {
            holds: Arc::default(),
            state: Arc::new(Mutex::new(State {
                checkpoint,
                source_revision: 1,
                groups: groups.iter().map(|(k, d)| (digest(*k), *d)).collect(),
                evidence: BTreeMap::new(),
                cold: BTreeMap::new(),
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
        assert!(s.checkpoint.pending.is_none() && s.checkpoint.page.is_none());
        s.groups.insert(digest(key), Decision::SettledDeleted);
        s.source_revision += 1;
    }
}

fn cas(s: &State, expected: &Checkpoint) -> Result<()> {
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
        Ok(self.state.lock().unwrap().checkpoint.clone())
    }
    async fn acquire(&self, expected: &Checkpoint) -> Result<Arc<ModelHold>> {
        let s = self.state.lock().unwrap();
        cas(&s, expected)?;
        if s.in_flight.is_some() {
            return Err(Refusal::Conflict);
        }
        let mut binding = s.checkpoint.binding;
        if s.wrong_authority {
            binding.epoch += 1;
        }
        self.holds.fetch_add(1, Ordering::SeqCst);
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
    async fn retain_evidence(
        &self,
        _: Arc<ModelHold>,
        expected: &Checkpoint,
        group: Digest,
    ) -> Result<Intent> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        if let Some(error) = s.unavailable {
            return Err(error);
        }
        let intent = if let Some(intent) = &s.staged {
            if intent.group != group
                || intent.binding != expected.binding
                || intent.before_revision != expected.source_revision
            {
                return Err(Refusal::Conflict);
            }
            intent.clone()
        } else {
            let operation = s.next_operation.to_be_bytes();
            s.next_operation += 1;
            let intent = Intent {
                operation,
                binding: expected.binding,
                group,
                before_revision: expected.source_revision,
                plan_sha256: digest(3),
                source_sha256: digest(4),
                observation_sha256: digest(5),
                evidence_sha256: digest(6),
                decision: *s.groups.get(&group).ok_or(Refusal::InvalidEvidence)?,
            };
            s.evidence.insert(operation, intent.clone());
            s.staged = Some(intent.clone());
            intent
        };
        fail_after(&mut s, Call::Evidence)?;
        Ok(intent)
    }
    async fn select(&self, expected: &Checkpoint, intent: Intent) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        if s.evidence.get(&intent.operation) != Some(&intent) {
            return Err(Refusal::InvalidEvidence);
        }
        s.selected.push(intent.operation);
        s.checkpoint.pending = Some(Pending {
            intent,
            receipts: vec![],
        });
        s.checkpoint.revision += 1;
        fail_after(&mut s, Call::Select)
    }
    async fn verify_retained(&self, intent: &Intent) -> Result<()> {
        let s = self.state.lock().unwrap();
        if let Some(error) = s.unavailable {
            return Err(error);
        }
        if s.evidence.get(&intent.operation) != Some(intent) {
            return Err(Refusal::InvalidEvidence);
        }
        Ok(())
    }
    async fn reconcile(
        &self,
        _: Arc<ModelHold>,
        intent: &Intent,
        phase: Phase,
    ) -> Result<Reconciled> {
        let s = self.state.lock().unwrap();
        if let Some(error) = s.unavailable {
            return Err(error);
        }
        if let Some(receipt) = s
            .cold
            .get(&intent.operation)
            .and_then(|r| r.iter().find(|r| r.phase == phase))
        {
            let mut receipt = receipt.clone();
            if s.corrupt_receipt {
                receipt.intent.plan_sha256 = digest(9999);
            }
            return Ok(Reconciled::OriginalReceipt(Box::new(receipt)));
        }
        if s.source_revision != intent.before_revision {
            return Err(Refusal::UnknownOutcome);
        }
        Ok(Reconciled::ResumeOriginal)
    }
    async fn collect_group(&self, hold: Arc<ModelHold>, intent: &Intent) -> Result<()> {
        let suspend = {
            let mut s = self.state.lock().unwrap();
            if let Some(error) = s.unavailable {
                return Err(error);
            }
            if s.cold.contains_key(&intent.operation) {
                return Err(Refusal::InvalidEvidence);
            }
            if s.source_revision != intent.before_revision {
                return Err(Refusal::StaleCursor);
            }
            if s.groups.remove(&intent.group) != Some(Decision::SettledDeleted) {
                return Err(Refusal::InvalidEvidence);
            }
            s.source_revision += 1;
            s.mutations += 1;
            // Model counts only; the real adapters must measure actual stores.
            s.retired_operations += 6;
            s.retired_authority += 3;
            let receipts = [Phase::Prepared, Phase::SourceCommitted, Phase::Acknowledged]
                .into_iter()
                .enumerate()
                .map(|(i, phase)| Receipt {
                    intent: intent.clone(),
                    phase,
                    after_revision: if i == 0 {
                        intent.before_revision
                    } else {
                        s.source_revision
                    },
                    sha256: digest(100 + i as u64),
                })
                .collect();
            s.cold.insert(intent.operation, receipts);
            fail_after(&mut s, Call::Dispatch)?;
            if s.suspend_dispatch {
                s.in_flight = Some(hold);
            }
            s.suspend_dispatch
        };
        if suspend {
            std::future::pending::<()>().await;
        }
        Ok(())
    }
    async fn record_receipt(&self, expected: &Checkpoint, receipt: Receipt) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        s.checkpoint
            .pending
            .as_mut()
            .unwrap()
            .receipts
            .push(receipt);
        s.checkpoint.revision += 1;
        fail_after(&mut s, Call::Record)
    }
    async fn settle(&self, expected: &Checkpoint) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        cas(&s, expected)?;
        let pending = s.checkpoint.pending.take().unwrap();
        s.checkpoint.after_group = Some(pending.intent.group);
        if pending.intent.decision == Decision::SettledDeleted {
            assert_eq!(pending.receipts.len(), 3);
            s.checkpoint.source_revision = pending.receipts[2].after_revision;
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
        assert!(s.checkpoint.pending.is_none());
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
