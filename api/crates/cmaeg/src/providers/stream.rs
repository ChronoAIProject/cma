//! Stream ownership ends when its consumer leaves, including while the socket is idle.
use super::{parse_event, parse_sse, response_id_from_frame, Accepted, ResponsesEvent};
use futures_util::StreamExt;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot, OwnedSemaphorePermit};

pub(super) async fn forward_turn(
    response: reqwest::Response,
    accepted_tx: oneshot::Sender<Accepted>,
    events_tx: mpsc::Sender<ResponsesEvent>,
    _permit: OwnedSemaphorePermit,
) {
    let mut accepted_tx = Some(accepted_tx);
    let mut stream = Box::pin(parse_sse(response.bytes_stream()));
    loop {
        let frame = tokio::select! {
            _ = events_tx.closed() => break,
            frame = stream.next() => match frame { Some(frame) => frame, None => break },
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, "provider SSE parser stopped");
                break;
            }
        };
        let event = match parse_event(&frame) {
            Ok(event) => event,
            Err(error) => {
                tracing::warn!(%error, "provider response event was rejected");
                break;
            }
        };
        if let Some(sender) = accepted_tx.take() {
            match &event {
                ResponsesEvent::Created { response_id } => {
                    let _ = sender.send(Accepted {
                        response_id: response_id.clone(),
                        at: Instant::now(),
                    });
                }
                ResponsesEvent::InProgress => {
                    let Some(response_id) =
                        response_id_from_frame(&frame).filter(|id| !id.is_empty())
                    else {
                        tracing::warn!("provider acceptance frame has no response id");
                        break;
                    };
                    let _ = sender.send(Accepted {
                        response_id,
                        at: Instant::now(),
                    });
                }
                _ => accepted_tx = Some(sender),
            }
        }
        let terminal = matches!(
            event,
            ResponsesEvent::Completed { .. }
                | ResponsesEvent::Failed { .. }
                | ResponsesEvent::Incomplete { .. }
        );
        if events_tx.send(event).await.is_err() {
            break;
        }
        if terminal {
            break;
        }
    }
}
