use std::{future::Future, pin::Pin, sync::Arc};

use axum::extract::ws::{Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use layer2_core::{
    AttachmentClaim, Caller, CoreError, ResourceReading, SessionId, SessionService,
    TerminalGeometry,
};

use super::{
    ending, error_code, reported_ending, reports_resources, supported_signal, ClientControl,
    Limits, ServerControl, CONFORMS_TO, KEEPALIVE_INTERVAL, MAX_FRAME_BYTES, SCROLLBACK_LINES,
};

/// One client frame already accepted from the socket and still inside core.
type InFlightOperation =
    Pin<Box<dyn Future<Output = (&'static str, Result<(), CoreError>)> + Send>>;

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

    let attachment = match service.attach_terminal(&caller, &id, geometry, claim).await {
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

    let process_ending = attachment.ending.clone();
    // Own the original receivers. Resubscribing here would discard output
    // queued while the ready frame and repaint were being sent, including a
    // fast process's final bytes immediately before its exit report.
    let mut attachment = attachment;
    // A terminal is mostly silent — someone reading output produces no traffic
    // at all — and an idle connection is exactly what proxies and load balancers
    // reclaim. Without this the session dies mid-thought for no reason the user
    // can see.
    let mut heartbeat = tokio::time::interval(KEEPALIVE_INTERVAL);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // The first tick is immediate and would send a ping before anything has had
    // a chance to go idle.
    heartbeat.tick().await;

    // One task owns both halves of the acceptance boundary and is the socket's
    // sole writer. The two slots below are the drain protocol: attached and
    // quiescent accepts one frame; attached and in-flight polls that operation;
    // superseded and in-flight drains it; superseded and quiescent writes the
    // close. Selecting a client frame installs its core future before this task
    // can observe supersession, while selecting supersession first disables the
    // source. Thus no second frame can cross the boundary.
    let mut in_flight: Option<InFlightOperation> = None;
    let mut supersession: Option<Option<layer2_core::Detached>> = None;
    loop {
        // A supersession close drains the one frame already accepted from the
        // socket. Its result is written below before this ending can be written.
        // With no frame in flight, an idle source cannot delay the close.
        if in_flight.is_none() {
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
        let operation_pending = in_flight.is_some();
        let accepts_input = attached && !operation_pending;
        tokio::select! {
            // Output remains ordered ahead of lifecycle endings. Supersession
            // remains ordered ahead of source receipt, which establishes the
            // deterministic input boundary described above.
            biased;
            completed = finish_operation(&mut in_flight), if operation_pending => {
                in_flight = None;
                let (operation, result) = completed;
                if let Err(error) = result {
                    let code = error_code(&error);
                    tracing::warn!(
                        session_id = %id,
                        %operation,
                        %code,
                        error = %error,
                        "terminal operation refused"
                    );
                    if sink
                        .send(
                            ServerControl::Error {
                                code,
                                detail: error.to_string(),
                            }
                            .to_message(),
                        )
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
            }
            received = attachment.output.recv(), if attached => match received {
                Ok(chunk) => {
                    if sink.send(Message::Binary(chunk.into())).await.is_err() {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "terminal client lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
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
                Err(tokio::sync::broadcast::error::RecvError::Lagged(frames)) => {
                    tracing::debug!(frames, "terminal client lagged on resource readings");
                }
                // Gauges went away, not the terminal. Keep every other branch
                // alive and retire only this lane.
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
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
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            }
        }
    }
    // Dropping the attachment releases the exclusive lease. The session keeps
    // running: a dropped connection detaches, it does not terminate.
}

/// Optional deterministic receipt boundary for composed interface tests.
async fn operation_received(id: &SessionId, operation: &'static str) {
    #[cfg(feature = "test-support")]
    super::test_support::operation_received(id, operation).await;

    #[cfg(not(feature = "test-support"))]
    let _ = (id, operation);
}

/// Poll the sole accepted operation while keeping the output lanes live.
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
