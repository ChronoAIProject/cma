//! Drive native reads and durable acknowledgements on one attachment lifetime.

use futures_util::{stream::SplitStream, StreamExt as _};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::{Error, Message};

use super::{
    gap_count, unframe, Bridge, Socket, LANE_CONVERSATION, LANE_CONVERSATION_GAP,
    LANE_RETAINED_OUTPUT,
};
use crate::hosted::update_relay::UpdateRelay;
use crate::{ConversationUpdate, RuntimeOutputSource};

pub(super) fn spawn(
    reader: Arc<Bridge>,
    mut source: SplitStream<Socket>,
    reader_updates: UpdateRelay,
    acknowledged: mpsc::Receiver<RuntimeOutputSource>,
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
                    reader.acknowledge_output(ack, state.as_ref()).await?;
                }
                Ok::<_, crate::AdapterError>(())
            });
        }
        loop {
            let frame = tokio::select! {
                _ = reader_updates.closed(), if reader_updates.bounded() => break,
                _ = ack_tasks.join_next(), if reader_updates.bounded() => break,
                frame = source.next() => match frame {
                    Some(frame) => frame,
                    None => break,
                },
                Some(ack) = legacy_ack(&mut acknowledged) => {
                    if reader.acknowledge_output(ack, ack_state.as_ref()).await.is_err() {
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

async fn dispatch(reader: &Bridge, frame: Result<Message, Error>, updates: &UpdateRelay) -> bool {
    match frame {
        Ok(Message::Binary(bytes)) => match unframe(&bytes) {
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
        Ok(Message::Close(_)) => return false,
        Ok(Message::Text(_)) => {
            tracing::debug!("text frame ignored after Ready declaration");
        }
        Ok(_) => {}
        Err(error) => {
            tracing::warn!(%error, "conversation link read failed");
            return false;
        }
    }
    true
}

async fn legacy_ack(
    receiver: &mut Option<mpsc::Receiver<RuntimeOutputSource>>,
) -> Option<RuntimeOutputSource> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}
