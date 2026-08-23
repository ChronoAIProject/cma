use std::sync::Arc;

use axum::extract::ws::{CloseFrame, Message, WebSocket};
use futures_util::{SinkExt, StreamExt};
use layer2_core::{AttachmentClaim, Caller, SessionId, SessionService, TerminalGeometry};

use super::{
    ending, error_code, ClientControl, Limits, ServerControl, CONFORMS_TO, KEEPALIVE_INTERVAL,
    MAX_FRAME_BYTES, SCROLLBACK_LINES,
};

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

    let _ = sink
        .send(
            ServerControl::Ready {
                cols: attachment.geometry.columns(),
                rows: attachment.geometry.rows(),
                snapshot: true,
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

    let mut output = attachment.output.resubscribe();
    let mut superseded = attachment.superseded.resubscribe();
    let mut pump_out = tokio::spawn(async move {
        // A terminal is mostly silent — someone reading output produces no
        // traffic at all — and an idle connection is exactly what proxies and
        // load balancers reclaim. Without this the session dies mid-thought
        // for no reason the user can see.
        //
        // A protocol-level ping rather than an application one, because
        // browsers answer it themselves: the reply proves the path is still
        // open in both directions without the page having to do anything.
        let mut heartbeat = tokio::time::interval(KEEPALIVE_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // The first tick is immediate and would send a ping before anything
        // has had a chance to go idle.
        heartbeat.tick().await;
        loop {
            tokio::select! {
                received = output.recv() => match received {
                    Ok(chunk) => {
                        if sink.send(Message::Binary(chunk.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "terminal client lagged");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                _ = heartbeat.tick() => {
                    if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                        break;
                    }
                }
                // This attachment has been taken away. Said rather than shown,
                // and said accurately: a terminal that simply stops updating is
                // indistinguishable from a broken one, and one that names the
                // wrong reason sends its owner somewhere there is nothing to
                // find.
                taken = superseded.recv() => {
                    let (code, reason) = ending(taken.ok());
                    let _ = sink
                        .send(Message::Close(Some(CloseFrame {
                            code,
                            reason: reason.into(),
                        })))
                        .await;
                    break;
                }
            }
        }
    });

    let svc = Arc::clone(&service);
    let sid = id.clone();
    let caller_in = caller.clone();
    let mut pump_in = tokio::spawn(async move {
        while let Some(frame) = source.next().await {
            match frame {
                Ok(Message::Binary(bytes)) => {
                    if bytes.len() as u32 > MAX_FRAME_BYTES {
                        tracing::warn!(session_id = %sid, len = bytes.len(), "oversized frame");
                        break;
                    }
                    if svc
                        .send_input(&caller_in, &sid, bytes.to_vec())
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(Message::Text(text)) => match serde_json::from_str::<ClientControl>(&text) {
                    Ok(ClientControl::Resize { cols, rows }) => {
                        match TerminalGeometry::new(cols, rows) {
                            Ok(g) => {
                                let _ = svc.resize_terminal(&caller_in, &sid, g).await;
                            }
                            Err(e) => tracing::debug!(error = %e, "zero geometry ignored"),
                        }
                    }
                    Ok(ClientControl::Signal { sig }) => {
                        let _ = svc.send_signal(&caller_in, &sid, sig).await;
                    }
                    // Answered by the heartbeat above rather than here: this
                    // half of the socket cannot write. A client ping still
                    // serves its purpose, since the traffic itself is what
                    // keeps an intermediary from reclaiming the connection.
                    Ok(ClientControl::Ping) => {}
                    Err(e) => tracing::debug!(error = %e, "malformed control ignored"),
                },
                Ok(Message::Close(_)) => break,
                Ok(_) => {}
                Err(_) => break,
            }
        }
    });

    tokio::select! {
        _ = &mut pump_out => pump_in.abort(),
        _ = &mut pump_in => pump_out.abort(),
    }
    // Dropping the attachment releases the exclusive lease. The session keeps
    // running: a dropped connection detaches, it does not terminate.
    drop(attachment);
}
