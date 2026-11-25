//! Stream ownership ends when its consumer leaves, including while the socket is idle.
use super::{
    client_types::StreamEvent, parse_sse, response_id_from_frame, Accepted, ResponsesEvent,
};
use futures_util::StreamExt;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};

pub(super) async fn forward_turn(
    response: reqwest::Response,
    accepted_tx: oneshot::Sender<Accepted>,
    events_tx: mpsc::Sender<StreamEvent>,
    _permit: super::permits::Permit,
    complete: super::transport::Completion,
    mut trace: Option<Box<crate::telemetry::turn_trace::ProviderReader>>,
    association: Option<
        std::sync::Arc<std::sync::OnceLock<crate::telemetry::turn_trace::Association>>,
    >,
) {
    let mut accepted_tx = Some(accepted_tx);
    let mut accepted_response: Option<String> = None;
    let mut consistent_identity = true;
    let mut stream = Box::pin(parse_sse(response.bytes_stream()));
    loop {
        let frame = tokio::select! {
            _ = events_tx.closed() => break,
            frame = stream.next() => match frame { Some(frame) => frame, None => break },
        };
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(kind = error.kind(), "provider SSE parser stopped");
                break;
            }
        };
        let (event, matches_identity) = match super::responses::parse_event_for_turn(
            &frame,
            trace.as_deref_mut(),
            accepted_response.as_deref(),
        ) {
            Ok(event) => event,
            Err(_) => {
                tracing::warn!("provider response event was rejected");
                break;
            }
        };
        if let Some(slot) = &association {
            if slot.get().is_none() {
                if let Some(value) = trace.as_ref().and_then(|reader| reader.association()) {
                    let _ = slot.set(value);
                }
            }
        }
        if let Some(sender) = accepted_tx.take() {
            match &event {
                ResponsesEvent::Created {
                    response_id,
                    thread_reply,
                    ..
                } => {
                    if response_id.is_empty() || response_id.len() > 512 {
                        break;
                    }
                    accepted_response = Some(response_id.clone());
                    let _ = sender.send(Accepted {
                        response_id: response_id.clone(),
                        thread_reply: *thread_reply,
                        at: Instant::now(),
                    });
                }
                ResponsesEvent::InProgress { .. } => {
                    let Some(response_id) = response_id_from_frame(&frame)
                        .filter(|id| !id.is_empty() && id.len() <= 512)
                    else {
                        tracing::warn!("provider acceptance frame has no response id");
                        break;
                    };
                    accepted_response = Some(response_id.clone());
                    let _ = sender.send(Accepted {
                        thread_reply: false,
                        response_id,
                        at: Instant::now(),
                    });
                }
                _ => accepted_tx = Some(sender),
            }
        }
        // Projection can reuse text from earlier frames. A contradictory identity
        // or an unscoped text fragment disqualifies subsequent Answer evidence.
        let unscoped_output = accepted_response.is_none()
            && matches!(
                event,
                ResponsesEvent::OutputTextDelta { .. }
                    | ResponsesEvent::OutputTextDone { .. }
                    | ResponsesEvent::OutputItemAdded { .. }
                    | ResponsesEvent::OutputItemDone { .. }
            );
        if consistent_identity && (!matches_identity || unscoped_output) {
            tracing::warn!(
                "provider stream did not establish a consistent answer response identity"
            );
            consistent_identity = false;
        }
        let terminal = matches!(
            event,
            ResponsesEvent::Completed { .. }
                | ResponsesEvent::Failed { .. }
                | ResponsesEvent::Incomplete { .. }
        );
        if events_tx
            .send(StreamEvent {
                event,
                bound_to_accepted: consistent_identity && accepted_response.is_some(),
            })
            .await
            .is_err()
        {
            break;
        }
        if terminal {
            complete.complete();
            break;
        }
    }
}
