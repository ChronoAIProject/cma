//! Drive native reads and durable acknowledgements on one attachment lifetime.

use futures_util::StreamExt as _;
use std::sync::Arc;
use tokio_tungstenite::tungstenite::{Error, Message};

use super::ingress::{Ack, AckReceiver, BridgeSource, IncomingFrame};
use super::{
    gap_count, unframe, Bridge, LANE_CONVERSATION, LANE_CONVERSATION_GAP, LANE_RETAINED_OUTPUT,
};
use crate::hosted::update_relay::UpdateRelay;
use crate::ConversationUpdate;

pub(super) fn spawn(
    reader: Arc<Bridge>,
    mut source: BridgeSource,
    reader_updates: UpdateRelay,
    acknowledged: AckReceiver,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ack_state = reader.output_ack_state().await;
        let mut acknowledged = Some(acknowledged);
        // JoinSet aborts its child if the reader is cancelled during delivery.
        // The ACK writer must progress independently even when dispatch owns the
        // shared sink while waiting for socket readiness.
        let mut ack_tasks = tokio::task::JoinSet::new();
        if reader_updates.bounded() {
            let reader = reader.clone();
            let state = ack_state.take();
            let mut acknowledgements = acknowledged.take().unwrap();
            ack_tasks.spawn(async move {
                while let Some(ack) = acknowledgements.recv().await {
                    if !acknowledge(&reader, ack, state.as_ref()).await {
                        return;
                    }
                }
            });
        }
        loop {
            let frame = tokio::select! {
                biased;
                _ = reader.owner_revoked() => break,
                _ = reader_updates.closed(), if reader_updates.bounded() => break,
                _ = ack_tasks.join_next(), if reader_updates.bounded() => break,
                frame = source.next() => match frame {
                    Some(frame) => frame,
                    None => break,
                },
                Some(ack) = legacy_ack(&mut acknowledged) => {
                    if !acknowledge(&reader, ack, ack_state.as_ref()).await {
                        break;
                    }
                    continue;
                }
            };
            let delivery = dispatch(&reader, frame, &reader_updates);
            tokio::pin!(delivery);
            let delivered = if reader_updates.bounded() {
                tokio::select! {
                    _ = reader_updates.closed() => false,
                    _ = ack_tasks.join_next() => false,
                    result = &mut delivery => result,
                }
            } else {
                delivery.await
            };
            if !delivered {
                break;
            }
        }
        ack_tasks.shutdown().await;
        reader.finish().await;
    })
}

pub(super) async fn dispatch(
    reader: &Bridge,
    frame: Result<IncomingFrame, Error>,
    updates: &UpdateRelay,
) -> bool {
    with_diagnostics(reader, dispatch_inner(reader, frame, updates)).await
}

pub(super) async fn acknowledge(
    reader: &Bridge,
    ack: Ack,
    state: Option<&super::output::OutputAckState>,
) -> bool {
    with_diagnostics(reader, async {
        reader.acknowledge_output(ack, state).await.is_ok()
    })
    .await
}

async fn with_diagnostics(
    reader: &Bridge,
    operation: impl std::future::Future<Output = bool>,
) -> bool {
    use crate::ingress_budget::{sizing, AllocationPlan, IngressClass, IngressWorkScope};
    let Some(policy) = &reader.ingress else {
        return operation.await;
    };
    // Reader and ACK tasks do not inherit the opening caller's task locals.
    // Keep refusal/close diagnostics independent of exhausted data capacity.
    let plan = AllocationPlan::checked(
        "reader diagnostic scope",
        &[sizing::shared_owner_bytes::<()>()],
    )
    .expect("fixed diagnostic scope layout");
    let Ok(storage) = policy.admit(IngressClass::Control, plan) else {
        return false;
    };
    IngressWorkScope::new(storage)
        .initialize_owned(|| async { Ok(operation.await) })
        .resolve_admitted_result(policy, IngressClass::Control)
        .await
        .is_ok_and(|completed| *completed)
}

async fn dispatch_inner(
    reader: &Bridge,
    frame: Result<IncomingFrame, Error>,
    updates: &UpdateRelay,
) -> bool {
    let frame = match frame {
        Ok(frame) => frame,
        Err(error) => {
            tracing::warn!(%error, "conversation link read failed");
            return false;
        }
    };
    match &*frame {
        Message::Binary(bytes) => match unframe(bytes) {
            Some((LANE_CONVERSATION, payload)) => {
                if reader.output.is_some() {
                    tracing::error!("negotiated runtime output arrived without a receipt");
                    return false;
                }
                if let Err(error) = reader.receive(payload, updates).await {
                    tracing::error!(%error, "runtime conversation was rejected");
                    return false;
                }
            }
            Some((LANE_RETAINED_OUTPUT, payload)) => {
                if let Err(error) = reader.receive_output(payload, updates).await {
                    tracing::error!(%error, "retained runtime output was rejected");
                    return false;
                }
            }
            Some((LANE_CONVERSATION_GAP, payload)) => {
                let frames = match &reader.json_pools {
                    Some(pools) => match pools.decode(payload) {
                        Ok(frame) => frame.get("frames").and_then(serde_json::Value::as_u64),
                        Err(_) => return false,
                    },
                    None => gap_count(payload),
                };
                if let Some(frames) = frames {
                    updates.send(ConversationUpdate::Lost { frames }).await;
                }
                if reader.output.is_some() {
                    return false;
                }
            }
            _ => {}
        },
        Message::Close(_) => return false,
        Message::Text(_) => {
            tracing::debug!("text frame ignored after Ready declaration");
        }
        _ => {}
    }
    true
}

async fn legacy_ack(receiver: &mut Option<AckReceiver>) -> Option<Ack> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}
