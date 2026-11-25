use super::*;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Behavior {
    pub unavailable: bool,
    pub stall: bool,
    pub wrong_audience: bool,
    pub wrong_kind: bool,
    pub local_writer: bool,
    pub transport_open: bool,
    pub bad_watermark: bool,
    pub bad_high_water: bool,
    pub lose_reply: bool,
}

pub(super) struct Endpoint {
    pub row: ExecutionRecordV1,
    pub behavior: Mutex<Behavior>,
    pub retained: Mutex<Option<MemberCompletionV1>>,
    pub calls: Mutex<Vec<&'static str>>,
    pub query_gate: Mutex<Option<Arc<tokio::sync::Barrier>>>,
    pub stop_gate: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
}

impl Endpoint {
    async fn before_stop(&self) {
        let gate = self.stop_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.acquire().await.unwrap().forget();
        }
    }
    pub fn barrier(&self, request: &wire::BarrierRequestV1) -> wire::BarrierReceiptV1 {
        wire::BarrierReceiptV1 {
            protocol: 1,
            request: request.clone(),
            barrier_sequence: generation(2),
            durable_watermark: if self.behavior.lock().unwrap().bad_watermark {
                generation(99)
            } else {
                request.revocation.watermark
            },
        }
    }
    pub fn completion(&self, request: &wire::BarrierRequestV1) -> MemberCompletionV1 {
        let barrier = self.barrier(request);
        let behavior = self.behavior.lock().unwrap();
        match self.row.member {
            wire::MemberKey::Execution { .. } => MemberCompletionV1::Execution {
                receipt: wire::StopReceiptV1 {
                    protocol: 1,
                    barrier,
                    stop_sequence: generation(3),
                    evidence: wire::StopEvidenceKind::RejectedBeforeStart,
                    quiescence: wire::QuiescenceV1 {
                        queues_purged: true,
                        descendants_absent: true,
                        output_closed: true,
                        submitters_closed: true,
                        pending_local_effects_settled: !behavior.local_writer,
                        local_changes_digest: digest('c'),
                        effects: wire::EffectJournalV1 {
                            high_water: u64::from(self.row.effect_count)
                                + u64::from(behavior.bad_high_water),
                            digest: digest('d'),
                            unresolved_count: self.row.effect_count,
                        },
                    },
                },
            },
            wire::MemberKey::Release { .. } => MemberCompletionV1::Release {
                receipt: wire::ReleaseReceiptV1 {
                    protocol: 1,
                    barrier,
                    close_sequence: generation(3),
                    producers_closed: true,
                    pending_chunks_discarded: true,
                    transport_drained: !behavior.transport_open,
                    released_bytes: 19,
                },
            },
        }
    }
    async fn query(&self) -> ExecutionResult<Option<MemberCompletionV1>> {
        self.calls.lock().unwrap().push("query");
        let (unavailable, stall) = {
            let behavior = self.behavior.lock().unwrap();
            (behavior.unavailable, behavior.stall)
        };
        if unavailable {
            return Err(ExecutionError::Unavailable);
        }
        if stall {
            std::future::pending::<()>().await;
        }
        let retained = self.retained.lock().unwrap().clone();
        let gate = self.query_gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate.wait().await;
        }
        Ok(retained)
    }
    fn stop(&self, request: &wire::BarrierRequestV1) -> ExecutionResult<MemberCompletionV1> {
        self.calls.lock().unwrap().push("stop");
        let completion = self.completion(request);
        *self.retained.lock().unwrap() = Some(completion.clone());
        let mut behavior = self.behavior.lock().unwrap();
        if std::mem::take(&mut behavior.lose_reply) {
            return Err(ExecutionError::Unknown);
        }
        Ok(completion)
    }
}

#[async_trait::async_trait]
impl wire::RuntimeExecutionControl for Endpoint {
    async fn stop_receipt(
        &self,
        _: &wire::BarrierRequestV1,
    ) -> wire::ProtocolResult<Option<wire::StopReceiptV1>> {
        self.query()
            .await
            .map(|value| {
                value.map(|value| match value {
                    MemberCompletionV1::Execution { receipt } => receipt,
                    _ => panic!(),
                })
            })
            .map_err(|_| wire::ProtocolError::Unavailable)
    }
    async fn install_barrier(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> wire::ProtocolResult<wire::BarrierReceiptV1> {
        self.calls.lock().unwrap().push("barrier");
        Ok(self.barrier(request))
    }
    async fn cancel(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> wire::ProtocolResult<wire::StopReceiptV1> {
        self.before_stop().await;
        match self
            .stop(request)
            .map_err(|_| wire::ProtocolError::Unknown)?
        {
            MemberCompletionV1::Execution { receipt } => Ok(receipt),
            _ => panic!(),
        }
    }
}

#[async_trait::async_trait]
impl ResourceReleaseGate for Endpoint {
    async fn receipt(
        &self,
        _: &wire::BarrierRequestV1,
    ) -> ExecutionResult<Option<wire::ReleaseReceiptV1>> {
        Ok(self.query().await?.map(|value| match value {
            MemberCompletionV1::Release { receipt } => receipt,
            _ => panic!(),
        }))
    }
    async fn install_barrier(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<wire::BarrierReceiptV1> {
        self.calls.lock().unwrap().push("barrier");
        Ok(self.barrier(request))
    }
    async fn close(
        &self,
        request: &wire::BarrierRequestV1,
    ) -> ExecutionResult<wire::ReleaseReceiptV1> {
        self.before_stop().await;
        match self.stop(request)? {
            MemberCompletionV1::Release { receipt } => Ok(receipt),
            _ => panic!(),
        }
    }
}

pub(super) struct Directory(pub BTreeMap<String, Arc<Endpoint>>);
impl Directory {
    pub fn new(rows: &[ExecutionRecordV1]) -> Arc<Self> {
        Arc::new(Self(
            rows.iter()
                .map(|row| {
                    (
                        row.registration.as_str().into(),
                        Arc::new(Endpoint {
                            row: row.clone(),
                            behavior: Mutex::new(Behavior::default()),
                            retained: Mutex::new(None),
                            calls: Mutex::new(Vec::new()),
                            query_gate: Mutex::new(None),
                            stop_gate: Mutex::new(None),
                        }),
                    )
                })
                .collect(),
        ))
    }
    pub fn endpoint(&self, ordinal: u32) -> &Endpoint {
        self.0
            .values()
            .find(|value| value.row.ordinal == ordinal)
            .unwrap()
    }
}

#[async_trait::async_trait]
impl ParticipantDirectory for Directory {
    async fn resolve(&self, member: &wire::MemberKey) -> ExecutionResult<Participant> {
        let control = self
            .0
            .get(registration_id(member)?.as_str())
            .ok_or(ExecutionError::Unavailable)?
            .clone();
        let mut audience = control.row.member.receiver().clone();
        let wrong_kind = {
            let behavior = control.behavior.lock().unwrap();
            if behavior.wrong_audience {
                audience.boot = id("other-boot");
            }
            behavior.wrong_kind
        };
        Ok(match member {
            wire::MemberKey::Execution { .. } if !wrong_kind => {
                Participant::execution(audience, control)
            }
            wire::MemberKey::Release { .. } if wrong_kind => {
                Participant::execution(audience, control)
            }
            _ => Participant::release(audience, control),
        })
    }
}
