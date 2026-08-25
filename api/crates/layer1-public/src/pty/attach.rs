use std::{future::Future, pin::Pin, sync::Arc};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use layer2_core::{
    AttachmentClaim, Caller, CoreError, ResourceReading, SessionId, SessionService,
    TerminalGeometry,
};
use tokio::sync::{broadcast, mpsc, oneshot};

use super::{
    ending, error_code, reported_ending, reports_resources, supported_signal, ClientControl,
    Limits, ServerControl, CONFORMS_TO, KEEPALIVE_INTERVAL, MAX_FRAME_BYTES, SCROLLBACK_LINES,
};

/// One client frame already accepted from the socket and still inside core.
type InFlightOperation =
    Pin<Box<dyn Future<Output = (&'static str, Result<(), CoreError>)> + Send>>;

/// A typed operation refusal and confirmation from the sole socket writer.
struct Refusal {
    control: ServerControl,
    delivered: oneshot::Sender<bool>,
}

/// What the independently polled socket reader needs from its writer.
enum InputEvent {
    Refusal(Refusal),
    /// Supersession was observed and the sole accepted operation has completed.
    Drained,
}

/// Keeps an input pump from outliving cancellation of its owning attachment.
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub(super) async fn serve(
    socket: WebSocket,
    service: Arc<SessionService>,
    id: SessionId,
    caller: Caller,
    claim: AttachmentClaim,
) {
    let (mut sink, mut source) = socket.split();

    // The client must declare its geometry before any terminal byte is sent.
    // A full-screen application painted at the wrong size produces corruption
    // that does not self-heal until the next full repaint, so the first paint
    // has to be at the client's real size.
    let geometry = loop {
        match source.next().await {
            Some(Ok(Message::Text(text))) => {
                match serde_json::from_str::<ClientControl>(&text) {
                    Ok(ClientControl::Resize { cols, rows }) => {
                        match TerminalGeometry::new(cols, rows) {
                            Ok(g) => break g,
                            Err(e) => {
                                let _ = sink
                                    .send(
                                        ServerControl::Error {
                                            code: "invalid_geometry",
                                            detail: e.to_string(),
                                        }
                                        .to_message(),
                                    )
                                    .await;
                                // A bad geometry is ignored, not fatal: the
                                // client is mid-layout and will send another.
                            }
                        }
                    }
                    Ok(ClientControl::Ping) => {
                        let _ = sink.send(ServerControl::Pong.to_message()).await;
                    }
                    Ok(ClientControl::Signal { .. }) => {
                        let _ = sink
                            .send(
                                ServerControl::Error {
                                    code: "malformed_control",
                                    detail: "resize must precede any other frame".into(),
                                }
                                .to_message(),
                            )
                            .await;
                    }
                    Err(e) => {
                        let _ = sink
                            .send(
                                ServerControl::Error {
                                    code: "malformed_control",
                                    detail: e.to_string(),
                                }
                                .to_message(),
                            )
                            .await;
                    }
                }
            }
            Some(Ok(Message::Binary(_))) => {
                let _ = sink
                    .send(
                        ServerControl::Error {
                            code: "malformed_control",
                            detail: "resize must precede the first data frame".into(),
                        }
                        .to_message(),
                    )
                    .await;
            }
            Some(Ok(_)) => {}
            Some(Err(_)) | None => return,
        }
    };

    let mut attachment = match service.attach_terminal(&caller, &id, geometry, claim).await {
        Ok(a) => a,
        Err(e) => {
            let code = error_code(&e);
            tracing::info!(session_id = %id, %code, "attach refused");
            let _ = sink
                .send(
                    ServerControl::Error {
                        code,
                        detail: e.to_string(),
                    }
                    .to_message(),
                )
                .await;
            let _ = sink.send(Message::Close(None)).await;
            return;
        }
    };

    // Give the input pump its own view before any further await. Checking the
    // original immediately afterwards covers the narrow race in which the
    // one-shot supersession was sent just before `resubscribe` took its cursor.
    let input_superseded = attachment.superseded.resubscribe();
    let initial_supersession = try_supersession(&mut attachment.superseded);

    // Open the lane before announcing it so a reading cannot fall between the
    // ready frame and the receiver. A granted session whose current runtime
    // cannot report still subscribes successfully, so the point read is what
    // distinguishes a lane that will never speak from one awaiting a sample.
    let mut resources = service.subscribe_resources(&caller, &id).await.ok();
    let has_resources = if resources.is_some() {
        reports_resources(true, service.last_reading(&caller, &id).await)
    } else {
        false
    };

    let _ = sink
        .send(
            ServerControl::Ready {
                cols: attachment.geometry.columns(),
                rows: attachment.geometry.rows(),
                snapshot: true,
                resources: has_resources,
                conforms_to: CONFORMS_TO,
                limits: Limits {
                    scrollback_lines: SCROLLBACK_LINES,
                    max_frame_bytes: MAX_FRAME_BYTES,
                },
            }
            .to_message(),
        )
        .await;

    // The snapshot repaints the current screen. It is sent as binary because it
    // is terminal bytes, not structure.
    if !attachment.repaint.is_empty() {
        let _ = sink
            .send(Message::Binary(attachment.repaint.clone().into()))
            .await;
    }

    // The WebSocket remains duplex under sustained output: the reader owns the
    // acceptance boundary in its own task, while this task remains the sole
    // writer. Refusals carry an acknowledgement, so the reader cannot declare
    // its accepted operation drained until the typed error is on the wire.
    let (input_events, mut input_event) = mpsc::channel::<InputEvent>(1);
    let input_task = tokio::spawn(pump_input(
        source,
        input_superseded,
        Arc::clone(&service),
        id.clone(),
        caller.clone(),
        input_events,
        initial_supersession.is_some(),
    ));
    let _abort_input_on_drop = AbortOnDrop(input_task.abort_handle());

    let process_ending = attachment.ending.clone();
    // Own the original receivers. Resubscribing here would discard output
    // queued while the ready frame and repaint were being sent, including a
    // fast process's final bytes immediately before its exit report.
    //
    // A terminal is mostly silent — someone reading output produces no traffic
    // at all — and an idle connection is exactly what proxies and load balancers
    // reclaim. Without this the session dies mid-thought for no reason the user
    // can see.
    let mut heartbeat = tokio::time::interval(KEEPALIVE_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick is immediate and would send a ping before anything has had
    // a chance to go idle.
    heartbeat.tick().await;

    let mut supersession = initial_supersession;
    let mut input_drained = false;
    let mut input_events_open = true;
    loop {
        // A lifecycle close is permitted only after the input task has stopped
        // accepting and has completed the one operation already across its
        // boundary. Any refusal was acknowledged by this writer before Drained.
        if input_drained {
            if let Some(taken) = supersession.take() {
                for frame in ending(taken) {
                    if sink.send(frame).await.is_err() {
                        break;
                    }
                }
                break;
            }
        }

        let attached = supersession.is_none();
        let has_resources = attached && resources.is_some();
        tokio::select! {
            // Refusals remain ahead of output, and output retains its ordering
            // ahead of resources, heartbeat, and lifecycle endings.
            biased;
            event = input_event.recv(), if input_events_open => match event {
                Some(InputEvent::Refusal(refusal)) => {
                    let delivered = sink.send(refusal.control.to_message()).await.is_ok();
                    let _ = refusal.delivered.send(delivered);
                    if !delivered {
                        break;
                    }
                }
                Some(InputEvent::Drained) => input_drained = true,
                None => {
                    input_events_open = false;
                    // A reader that ended without Drained observed a client-side
                    // close, socket failure, or malformed oversized frame.
                    if !input_drained {
                        break;
                    }
                }
            },
            received = attachment.output.recv(), if attached => match received {
                Ok(chunk) => {
                    outbound_blocked(&id).await;
                    if sink.send(Message::Binary(chunk.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "terminal client lagged");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    // The core records an exit before closing this sender, and
                    // broadcast drains queued bytes first. A rotation records
                    // its attachment ending before removing the old live entry.
                    match attachment.superseded.try_recv() {
                        Ok(taken) => supersession = Some(Some(taken)),
                        Err(_) => {
                            // No record means the runtime link was merely lost,
                            // so no process ending is fabricated.
                            let frames = reported_ending(*process_ending.borrow());
                            for frame in frames {
                                if sink.send(frame).await.is_err() {
                                    break;
                                }
                            }
                            break;
                        }
                    }
                }
            },
            received = receive_resource(&mut resources), if has_resources => match received {
                Ok(reading) => {
                    if sink
                        .send(ServerControl::from_reading(reading).to_message())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(frames)) => {
                    tracing::debug!(frames, "terminal client lagged on resource readings");
                }
                // Gauges went away, not the terminal. Keep every other branch
                // alive and retire only this lane.
                Err(broadcast::error::RecvError::Closed) => {
                    resources = None;
                }
            },
            _ = heartbeat.tick(), if attached => {
                // Protocol-level rather than application-level: browsers answer
                // without the page doing anything.
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
            }
            taken = attachment.superseded.recv(), if attached => {
                supersession = Some(taken.ok());
            }
        }
    }

    input_task.abort();
    let _ = input_task.await;
    // Dropping the attachment releases the exclusive lease. The session keeps
    // running: a dropped connection detaches, it does not terminate.
}

/// Poll client input independently of every potentially backpressured write.
async fn pump_input(
    mut source: futures_util::stream::SplitStream<WebSocket>,
    mut superseded: broadcast::Receiver<layer2_core::Detached>,
    service: Arc<SessionService>,
    id: SessionId,
    caller: Caller,
    events: mpsc::Sender<InputEvent>,
    initially_superseded: bool,
) {
    // These two slots are the acceptance protocol. Attached and quiescent may
    // select one frame. Attached and in-flight polls only that operation and
    // supersession. Superseded and in-flight drains it; superseded and quiescent
    // reports Drained. The biased supersession branch prevents a queued second
    // frame from crossing after the ending is observable here.
    let mut in_flight: Option<InFlightOperation> = None;
    let mut supersession = initially_superseded;
    loop {
        if supersession && in_flight.is_none() {
            let _ = events.send(InputEvent::Drained).await;
            break;
        }

        let operation_pending = in_flight.is_some();
        let accepts_input = !supersession && !operation_pending;
        tokio::select! {
            biased;
            completed = finish_operation(&mut in_flight), if operation_pending => {
                in_flight = None;
                let (operation, result) = completed;
                if let Err(error) = result {
                    if !report_refusal(&events, &id, operation, error).await {
                        break;
                    }
                }
            }
            _ = superseded.recv(), if !supersession => {
                supersession = true;
            }
            frame = source.next(), if accepts_input => match frame {
                Some(Ok(Message::Binary(bytes))) => {
                    if bytes.len() as u32 > MAX_FRAME_BYTES {
                        tracing::warn!(session_id = %id, len = bytes.len(), "oversized frame");
                        break;
                    }
                    let service = Arc::clone(&service);
                    let caller = caller.clone();
                    let id = id.clone();
                    in_flight = Some(Box::pin(async move {
                        operation_received(&id, "input").await;
                        let result = service.send_input(&caller, &id, bytes.to_vec()).await;
                        ("input", result)
                    }));
                }
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<ClientControl>(&text) {
                        Ok(ClientControl::Resize { cols, rows }) => {
                            match TerminalGeometry::new(cols, rows) {
                                Ok(geometry) => {
                                    let service = Arc::clone(&service);
                                    let caller = caller.clone();
                                    let id = id.clone();
                                    in_flight = Some(Box::pin(async move {
                                        operation_received(&id, "resize").await;
                                        let result = service
                                            .resize_terminal(&caller, &id, geometry)
                                            .await;
                                        ("resize", result)
                                    }));
                                }
                                Err(error) => tracing::debug!(%error, "zero geometry ignored"),
                            }
                        }
                        Ok(ClientControl::Signal { sig }) => {
                            if !supported_signal(&sig) {
                                tracing::warn!(
                                    session_id = %id,
                                    signal = %sig,
                                    "unknown signal rejected"
                                );
                                continue;
                            }
                            let service = Arc::clone(&service);
                            let caller = caller.clone();
                            let id = id.clone();
                            in_flight = Some(Box::pin(async move {
                                operation_received(&id, "signal").await;
                                let result = service.send_signal(&caller, &id, sig).await;
                                ("signal", result)
                            }));
                        }
                        // Client pings are traffic enough to keep an intermediary
                        // from reclaiming the connection. The heartbeat owns writes.
                        Ok(ClientControl::Ping) => {}
                        Err(error) => tracing::debug!(%error, "malformed control ignored"),
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                    source_closed(&id).await;
                    break;
                }
                Some(Ok(_)) => {}
            }
        }
    }
}

/// Hand one refused operation to the sole writer and wait until it is sent.
async fn report_refusal(
    events: &mpsc::Sender<InputEvent>,
    id: &SessionId,
    operation: &'static str,
    error: CoreError,
) -> bool {
    let code = error_code(&error);
    tracing::warn!(session_id = %id, %operation, %code, error = %error, "terminal operation refused");
    let (delivered, receipt) = oneshot::channel();
    if events
        .send(InputEvent::Refusal(Refusal {
            control: ServerControl::Error {
                code,
                detail: error.to_string(),
            },
            delivered,
        }))
        .await
        .is_err()
    {
        return false;
    }
    receipt.await.unwrap_or(false)
}

/// Consume a supersession already waiting on the writer's original receiver.
fn try_supersession(
    receiver: &mut broadcast::Receiver<layer2_core::Detached>,
) -> Option<Option<layer2_core::Detached>> {
    loop {
        match receiver.try_recv() {
            Ok(taken) => return Some(Some(taken)),
            Err(broadcast::error::TryRecvError::Empty) => return None,
            Err(broadcast::error::TryRecvError::Closed) => return Some(None),
            Err(broadcast::error::TryRecvError::Lagged(_)) => {}
        }
    }
}

/// Optional deterministic receipt boundary for composed interface tests.
async fn operation_received(id: &SessionId, operation: &'static str) {
    #[cfg(feature = "test-support")]
    super::test_support::operation_received(id, operation).await;

    #[cfg(not(feature = "test-support"))]
    let _ = (id, operation);
}

/// Optional deterministic blocked-writer boundary for composed tests.
async fn outbound_blocked(id: &SessionId) {
    #[cfg(feature = "test-support")]
    super::test_support::outbound_blocked(id).await;

    #[cfg(not(feature = "test-support"))]
    let _ = id;
}

/// Optional deterministic source-close boundary for composed tests.
async fn source_closed(id: &SessionId) {
    #[cfg(feature = "test-support")]
    super::test_support::source_closed(id).await;

    #[cfg(not(feature = "test-support"))]
    let _ = id;
}

/// Poll the sole accepted operation while keeping supersession observable.
async fn finish_operation(
    in_flight: &mut Option<InFlightOperation>,
) -> (&'static str, Result<(), CoreError>) {
    in_flight
        .as_mut()
        .expect("the select guard established an in-flight operation")
        .await
}

/// Receive from the optional lane whose select guard established its presence.
async fn receive_resource(
    receiver: &mut Option<tokio::sync::broadcast::Receiver<ResourceReading>>,
) -> Result<ResourceReading, tokio::sync::broadcast::error::RecvError> {
    receiver
        .as_mut()
        .expect("the select guard established this receiver")
        .recv()
        .await
}
