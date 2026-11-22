use super::*;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

pub(crate) struct GatedProvider {
    pub calls: Mutex<Vec<wire::EffectIntentV1>>,
    pub active: AtomicUsize,
    pub peak: AtomicUsize,
    pub unavailable: AtomicBool,
    pub gate: tokio::sync::Semaphore,
    entered: tokio::sync::Notify,
}

impl GatedProvider {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            active: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            unavailable: AtomicBool::new(false),
            gate: tokio::sync::Semaphore::new(0),
            entered: tokio::sync::Notify::new(),
        })
    }

    pub async fn wait_for_call(&self) {
        loop {
            let entered = self.entered.notified();
            if !self.calls.lock().unwrap().is_empty() {
                return;
            }
            entered.await;
        }
    }
}

struct ActiveStatus<'a>(&'a AtomicUsize);

impl Drop for ActiveStatus<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl EffectStatusProvider for GatedProvider {
    async fn status(
        &self,
        intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<Option<ProviderEffectObservation>> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        let _active = ActiveStatus(&self.active);
        self.peak.fetch_max(active, Ordering::SeqCst);
        self.calls.lock().unwrap().push(intent.clone());
        self.entered.notify_one();
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(ExecutionError::Unavailable);
        }
        self.gate.acquire().await.unwrap().forget();
        Ok(Some(ProviderEffectObservation {
            intent: intent.clone(),
            outcome: wire::EffectOutcome::CommitConfirmed,
            evidence_digest: digest('c'),
        }))
    }
}

pub(crate) struct Provider {
    pub calls: Mutex<Vec<wire::EffectIntentV1>>,
    pub reply: Mutex<ExecutionResult<Option<ProviderEffectObservation>>>,
    pub stalled: Mutex<Option<wire::OpaqueId>>,
    pub gate: Mutex<Option<Arc<tokio::sync::Barrier>>>,
}

impl Provider {
    pub fn new(effect: &EffectRecordV1, outcome: wire::EffectOutcome) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            reply: Mutex::new(Ok(Some(ProviderEffectObservation {
                intent: effect.permission.intent.clone(),
                outcome,
                evidence_digest: digest('c'),
            }))),
            stalled: Mutex::new(None),
            gate: Mutex::new(None),
        })
    }
}

#[async_trait::async_trait]
impl EffectStatusProvider for Provider {
    async fn status(
        &self,
        intent: &wire::EffectIntentV1,
    ) -> ExecutionResult<Option<ProviderEffectObservation>> {
        self.calls.lock().unwrap().push(intent.clone());
        let stalled = self.stalled.lock().unwrap().as_ref() == Some(&intent.effect_id);
        if stalled {
            std::future::pending::<()>().await;
        }
        let gate = self.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        self.reply.lock().unwrap().clone()
    }
}

pub(super) fn effect(row: &ExecutionRecordV1, ordinal: u32) -> EffectRecordV1 {
    let wire::MemberKey::Execution { execution } = &row.member else {
        panic!()
    };
    let intent = wire::EffectIntentV1 {
        effect_id: id(&format!("effect-{ordinal}")),
        execution: execution.clone(),
        registration: row.registration.clone(),
        grant: row.basis.clone(),
        operation: wire::EffectOperationClass::ToolRequest,
        request_digest: digest('b'),
        provider_idempotency: Some(digest('d')),
    };
    let value = EffectRecordV1 {
        protocol: 1,
        permission: wire::EffectPermissionV1 {
            protocol: 1,
            permission_id: wire::Digest::new(&format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(&("resource-effect-permission-v1", &intent)).unwrap()
                )
            ))
            .unwrap(),
            intent,
            ledger_sequence: generation(u64::from(ordinal)),
        },
        observations: Vec::new(),
    };
    value.validate().unwrap();
    value
}

pub(super) fn setup(count: u32) -> (Arc<Store>, Arc<Provider>, EffectReconciler, wire::Digest) {
    let mut row = member(1);
    row.effect_count = count;
    let registration = row.registration.clone();
    let effects: Vec<_> = (1..=count).map(|n| effect(&row, n)).collect();
    let provider = Provider::new(&effects[0], wire::EffectOutcome::CommitConfirmed);
    let store = Store::new(vec![row]);
    store.0.lock().unwrap().effects = Some(effects);
    let reconciler = EffectReconciler::new(store.clone(), provider.clone());
    (store, provider, reconciler, registration)
}

pub(super) fn append(effect: &mut EffectRecordV1, outcome: wire::EffectOutcome) {
    effect
        .observe(&wire::EffectObservationV1 {
            protocol: 1,
            permission_id: effect.permission.permission_id.clone(),
            execution: effect.permission.intent.execution.clone(),
            effect_id: effect.permission.intent.effect_id.clone(),
            sequence: generation(effect.observations.len() as u64 + 1),
            outcome,
            evidence_digest: digest('c'),
        })
        .unwrap();
}
