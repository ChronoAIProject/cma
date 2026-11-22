use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteFault {
    Barrier,
    Completion,
    Finalize,
    Effect,
}

pub(super) struct State {
    pub operation: RevocationOperationV1,
    pub members: Vec<ExecutionRecordV1>,
    pub events: Vec<&'static str>,
    pub fault: Option<WriteFault>,
    pub finalizations: usize,
    pub pages: Vec<u32>,
    pub page_size: usize,
    pub missing_page: bool,
    pub duplicate_discovery: bool,
    pub extra_discovery: Vec<RevocationOperationV1>,
    pub effects: Option<Vec<EffectRecordV1>>,
    pub effect_pages: Vec<u32>,
    pub record_reads: usize,
    pub record_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub effect_page_gate: Option<(u32, Arc<tokio::sync::Semaphore>)>,
    pub operation_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub cohort_page_gate: Option<(u32, Arc<tokio::sync::Semaphore>)>,
    pub finalize_before_gate: Option<Arc<tokio::sync::Semaphore>>,
    pub finalize_reply_gate: Option<Arc<tokio::sync::Semaphore>>,
}

pub(super) struct Store(pub Mutex<State>);

impl Store {
    pub fn new(members: Vec<ExecutionRecordV1>) -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            operation: operation(members.len() as u32),
            members,
            events: Vec::new(),
            fault: None,
            finalizations: 0,
            pages: Vec::new(),
            page_size: 32,
            missing_page: false,
            duplicate_discovery: false,
            extra_discovery: Vec::new(),
            effects: None,
            effect_pages: Vec::new(),
            record_reads: 0,
            record_gate: None,
            effect_page_gate: None,
            operation_gate: None,
            cohort_page_gate: None,
            finalize_before_gate: None,
            finalize_reply_gate: None,
        })))
    }
    pub fn id(&self) -> wire::Digest {
        self.0.lock().unwrap().operation.key.operation_id.clone()
    }
}

fn ambiguous(state: &mut State, cut: WriteFault) -> ExecutionResult<()> {
    if state.fault == Some(cut) {
        state.fault = None;
        Err(ExecutionError::Unknown)
    } else {
        Ok(())
    }
}

#[async_trait::async_trait]
impl ResourceExecutionStore for Store {
    async fn activation(&self) -> ExecutionResult<ActivationStateV1> {
        Ok(ActivationStateV1::Activated {
            epoch: generation(1),
        })
    }
    async fn revocation(
        &self,
        id: &wire::Digest,
    ) -> ExecutionResult<Option<RevocationOperationV1>> {
        let (operation, gate) = {
            let mut state = self.0.lock().unwrap();
            state.events.push("read_operation");
            (
                (&state.operation.key.operation_id == id).then(|| state.operation.clone()),
                state.operation_gate.clone(),
            )
        };
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(operation)
    }
    async fn pending_revocations(
        &self,
        after: Option<&wire::Digest>,
        limit: u32,
    ) -> ExecutionResult<Vec<RevocationOperationV1>> {
        assert_eq!(limit, MAX_PAGE_MEMBERS);
        let state = self.0.lock().unwrap();
        let mut values = vec![state.operation.clone()];
        values.extend(state.extra_discovery.clone());
        values.retain(|row| {
            matches!(row.phase, RevocationPhaseV1::Pending)
                && after.is_none_or(|id| row.key.operation_id.as_str() > id.as_str())
        });
        values.sort_by(|a, b| a.key.operation_id.as_str().cmp(b.key.operation_id.as_str()));
        values.truncate(limit as usize);
        if state.duplicate_discovery && !values.is_empty() {
            values.push(values[0].clone());
        }
        Ok(values)
    }
    async fn frozen_members(
        &self,
        id: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<ExecutionRecordV1>> {
        let (rows, gate) = {
            let mut state = self.0.lock().unwrap();
            assert_eq!(id, &state.operation.key.operation_id);
            assert_eq!(limit, MAX_PAGE_MEMBERS);
            state.pages.push(after);
            if state.missing_page {
                return Ok(Vec::new());
            }
            (
                state
                    .members
                    .iter()
                    .skip(after as usize)
                    .take(state.page_size.min(limit as usize))
                    .cloned()
                    .collect(),
                state
                    .cohort_page_gate
                    .as_ref()
                    .filter(|(page, _)| *page == after)
                    .map(|(_, gate)| gate.clone()),
            )
        };
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(rows)
    }
    async fn record(
        &self,
        registration: &wire::Digest,
    ) -> ExecutionResult<Option<ExecutionRecordV1>> {
        let (record, gate) = {
            let mut state = self.0.lock().unwrap();
            state.record_reads += 1;
            (
                state
                    .members
                    .iter()
                    .find(|row| &row.registration == registration)
                    .cloned(),
                state.record_gate.clone(),
            )
        };
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(record)
    }
    async fn observe_barrier(&self, receipt: &wire::BarrierReceiptV1) -> ExecutionResult<()> {
        let mut state = self.0.lock().unwrap();
        let operation = state.operation.clone();
        let row = state
            .members
            .iter_mut()
            .find(|row| row.registration == receipt.request.registration)
            .unwrap();
        row.observe_barrier(&row.barrier_request(&operation)?, receipt)?;
        state.events.push("persist_barrier");
        ambiguous(&mut state, WriteFault::Barrier)
    }
    async fn observe_completion(&self, completion: &MemberCompletionV1) -> ExecutionResult<()> {
        let mut state = self.0.lock().unwrap();
        let barrier = completion_barrier(completion);
        let row = state
            .members
            .iter_mut()
            .find(|row| row.registration == barrier.request.registration)
            .unwrap();
        row.observe_completion(completion)?;
        state.events.push("persist_completion");
        ambiguous(&mut state, WriteFault::Completion)
    }
    async fn finish_revocation(&self, id: &wire::Digest) -> ExecutionResult<RevocationOperationV1> {
        let gate = self.0.lock().unwrap().finalize_before_gate.clone();
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        let (operation, gate) = {
            let mut state = self.0.lock().unwrap();
            assert_eq!(id, &state.operation.key.operation_id);
            state.finalizations += 1;
            state.operation.validate_cohort(&state.members)?;
            if matches!(state.operation.phase, RevocationPhaseV1::Pending) {
                state.operation.phase = RevocationPhaseV1::Effective {
                    receipt: Box::new(final_receipt(&state.operation)),
                };
            }
            state.operation.validate()?;
            state.events.push("finalize");
            ambiguous(&mut state, WriteFault::Finalize)?;
            (state.operation.clone(), state.finalize_reply_gate.clone())
        };
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(operation)
    }
    async fn grant(&self, _: &wire::Digest) -> ExecutionResult<Option<GrantControlV1>> {
        panic!("revocation worker cannot acquire authority")
    }
    async fn register(
        &self,
        _: &ExecutionAuthority,
        _: &wire::MemberKey,
    ) -> ExecutionResult<ExecutionRecordV1> {
        panic!("revocation worker cannot register new work")
    }
    async fn issue_ticket(
        &self,
        _: &ExecutionAuthority,
        _: &wire::ExecutionTicketV1,
    ) -> ExecutionResult<wire::ExecutionTicketV1> {
        panic!("revocation worker cannot issue tickets")
    }
    async fn continue_execution(
        &self,
        _: &ExecutionAuthority,
        _: &wire::ContinuationV1,
    ) -> ExecutionResult<wire::ContinuationV1> {
        panic!("revocation worker cannot continue execution")
    }
    async fn issue_release(
        &self,
        _: &ExecutionAuthority,
        _: &wire::ReleasePermitV1,
    ) -> ExecutionResult<wire::ReleasePermitV1> {
        panic!("revocation worker cannot release data")
    }
    async fn observe_start(&self, _: &wire::StartObservationV1) -> ExecutionResult<()> {
        panic!("revocation worker cannot start work")
    }
    async fn begin_revocation(
        &self,
        _: &RevocationAuthority,
    ) -> ExecutionResult<RevocationOperationV1> {
        panic!("recovery must retain the original operation")
    }
    async fn prepare_effect(
        &self,
        _: &wire::EffectIntentV1,
    ) -> ExecutionResult<wire::EffectPermissionV1> {
        panic!("revocation worker cannot prepare effects")
    }
    async fn observe_effect(&self, observation: &wire::EffectObservationV1) -> ExecutionResult<()> {
        let mut state = self.0.lock().unwrap();
        state
            .effects
            .as_mut()
            .expect("stop worker cannot invent effect outcomes")
            .iter_mut()
            .find(|effect| effect.permission.permission_id == observation.permission_id)
            .ok_or(ExecutionError::EvidenceMismatch)?
            .observe(observation)?;
        state.events.push("persist_effect");
        ambiguous(&mut state, WriteFault::Effect)
    }
    async fn effects_for(
        &self,
        registration: &wire::Digest,
        after: u32,
        limit: u32,
    ) -> ExecutionResult<Vec<EffectRecordV1>> {
        let (page, gate) = {
            let mut state = self.0.lock().unwrap();
            assert_eq!(registration, &state.members[0].registration);
            assert!(limit > 0 && limit <= MAX_PAGE_MEMBERS);
            state.effect_pages.push(after);
            (
                state
                    .effects
                    .as_ref()
                    .expect("effect reconciliation requires a status provider")
                    .iter()
                    .skip(after as usize)
                    .take(limit as usize)
                    .cloned()
                    .collect(),
                state
                    .effect_page_gate
                    .as_ref()
                    .filter(|(offset, _)| *offset == after)
                    .map(|(_, gate)| gate.clone()),
            )
        };
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
        Ok(page)
    }
}
