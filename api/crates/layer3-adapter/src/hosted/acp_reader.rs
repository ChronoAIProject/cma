//! Drive native reads and durable acknowledgements on one attachment lifetime.

use futures_util::{stream::SplitStream, StreamExt as _};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

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
    mut acknowledged: mpsc::Receiver<RuntimeOutputSource>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let frame = tokio::select! {
                biased;
                _ = reader.owner_revoked() => break,
                frame = source.next() => match frame {
                    Some(frame) => frame,
                    None => break,
                },
                Some(ack) = acknowledged.recv() => {
                    if reader.acknowledge_output(ack).await.is_err() {
                        break;
                    }
                    continue;
                }
            };
            match frame {
                Ok(Message::Binary(bytes)) => match unframe(&bytes) {
                    Some((LANE_CONVERSATION, payload)) => {
                        if reader.output.is_some() {
                            tracing::error!("negotiated runtime output arrived without a receipt");
                            break;
                        }
                        reader.receive(payload, &reader_updates).await;
                    }
                    Some((LANE_RETAINED_OUTPUT, payload)) => {
                        if let Err(error) = reader.receive_output(payload, &reader_updates).await {
                            tracing::error!(%error, "retained runtime output was rejected");
                            break;
                        }
                    }
                    Some((LANE_CONVERSATION_GAP, payload)) => {
                        if let Some(frames) = gap_count(payload) {
                            reader_updates.send(ConversationUpdate::Lost { frames });
                        }
                        if reader.output.is_some() {
                            break;
                        }
                    }
                    _ => {}
                },
                Ok(Message::Close(_)) => break,
                Ok(Message::Text(_)) => {
                    tracing::debug!("text frame ignored after Ready declaration");
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(%error, "conversation link read failed");
                    break;
                }
            }
        }
        reader.finish().await;
    })
}
