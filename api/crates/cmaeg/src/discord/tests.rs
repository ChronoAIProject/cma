use super::{gateway::socket_config, protocol::*, GatewayClient};
use cmaeg_core::channel::Secret;
use futures_util::{future::BoxFuture, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    io::DuplexStream,
    sync::{watch, Notify},
    task::JoinHandle,
};
use tokio_tungstenite::{
    tungstenite::{
        protocol::{frame::coding::CloseCode, CloseFrame, Role},
        Message,
    },
    WebSocketStream,
};

#[derive(Default)]
struct Journal {
    commits: Mutex<Vec<(u64, String)>>,
    admitted: Mutex<BTreeSet<String>>,
    fail_messages: AtomicBool,
    block_messages: AtomicBool,
    notice: Notify,
    unblock: Notify,
}
impl NativeDiscordJournal for Journal {
    fn authorize_connection<'a>(
        &'a self,
        _: Option<&'a Checkpoint>,
    ) -> BoxFuture<'a, Result<(), GatewayError>> {
        Box::pin(async { Ok(()) })
    }
    fn admit<'a>(
        &'a self,
        state: &'a Checkpoint,
        event: &'a Dispatch,
    ) -> BoxFuture<'a, Result<(), GatewayError>> {
        Box::pin(async move {
            if event.name == "MESSAGE_CREATE" {
                if self.fail_messages.load(Ordering::SeqCst) {
                    return Err(GatewayError::Unavailable);
                }
                if self.block_messages.load(Ordering::SeqCst) {
                    self.unblock.notified().await;
                }
                // Models durable unique native message identity across sessions.
                self.admitted
                    .lock()
                    .unwrap()
                    .insert(event.message_id().unwrap().into());
            }
            self.commits
                .lock()
                .unwrap()
                .push((state.sequence, event.name.clone()));
            self.notice.notify_waiters();
            Ok(())
        })
    }
}
impl Journal {
    async fn wait(&self, count: usize) {
        loop {
            let notified = self.notice.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.commits.lock().unwrap().len() >= count {
                return;
            }
            notified.await;
        }
    }
}
type Socket = WebSocketStream<DuplexStream>;
async fn start(
    journal: Arc<Journal>,
    resume: Option<Checkpoint>,
    jitter: u64,
) -> (Socket, watch::Sender<bool>, JoinHandle<GatewayEnd>) {
    let (left, right) = tokio::io::duplex(2 * FRAME_LIMIT);
    let client = WebSocketStream::from_raw_socket(left, Role::Client, Some(socket_config())).await;
    let server = WebSocketStream::from_raw_socket(right, Role::Server, None).await;
    let (stop, receiver) = watch::channel(false);
    let task = tokio::spawn(async move {
        GatewayClient::new(Secret::new("gateway-fixture"), "77".into(), MESSAGE_INTENTS)
            .unwrap()
            .run(client, resume, journal, receiver, jitter)
            .await
    });
    (server, stop, task)
}
async fn send(socket: &mut Socket, value: Value) {
    socket.send(Message::Text(value.to_string())).await.unwrap();
}
async fn read(socket: &mut Socket) -> Value {
    let Message::Text(value) = socket.next().await.unwrap().unwrap() else {
        panic!("expected JSON")
    };
    serde_json::from_str(&value).unwrap()
}
async fn hello(socket: &mut Socket) -> Value {
    send(socket, json!({"op":10,"d":{"heartbeat_interval":1000}})).await;
    read(socket).await
}
async fn ready(socket: &mut Socket) {
    send(socket,json!({"op":0,"s":0,"t":"READY","d":{"session_id":"session-fixture","resume_gateway_url":"wss://gateway-us-east1-b.discord.gg/","user":{"id":"77","bot":true}}})).await;
}
async fn message(socket: &mut Socket, sequence: u64, id: &str) {
    send(socket,json!({"op":0,"s":sequence,"t":"MESSAGE_CREATE","d":{"id":id,"channel_id":"42","author":{"id":"88"},"content":"hello"}})).await;
}

#[tokio::test]
async fn actual_frames_identify_admit_in_order_and_resume_without_duplicate_turns() {
    let journal = Arc::new(Journal::default());
    let (mut socket, _stop, task) = start(journal.clone(), None, 999).await;
    let identify = hello(&mut socket).await;
    assert_eq!(identify["op"], 2);
    assert_eq!(identify["d"]["intents"], MESSAGE_INTENTS);
    ready(&mut socket).await;
    message(&mut socket, 1, "100").await;
    journal.wait(2).await;
    send(&mut socket, json!({"op":7,"d":null})).await;
    let ended = task.await.unwrap();
    assert_eq!(ended.reason, EndReason::Reconnect);
    let checkpoint = ended.checkpoint.unwrap();
    assert_eq!(checkpoint.sequence, 1);
    let (mut socket, _stop, task) = start(journal.clone(), Some(checkpoint), 999).await;
    let resume = hello(&mut socket).await;
    assert_eq!(resume["op"], 6);
    assert_eq!(resume["d"]["seq"], 1);
    message(&mut socket, 1, "100").await; // Already checkpointed.
    message(&mut socket, 2, "100").await; // Unique native message record suppresses it.
    message(&mut socket, 3, "101").await;
    journal.wait(4).await;
    send(&mut socket, json!({"op":7,"d":null})).await;
    assert_eq!(task.await.unwrap().checkpoint.unwrap().sequence, 3);
    assert_eq!(journal.admitted.lock().unwrap().len(), 2);
}

#[tokio::test(start_paused = true)]
async fn heartbeat_uses_received_sequence_and_missing_ack_reconnects() {
    let journal = Arc::new(Journal::default());
    let (mut socket, _stop, task) = start(journal, None, 0).await;
    hello(&mut socket).await;
    assert_eq!(read(&mut socket).await, json!({"op":1,"d":null}));
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let ended = task.await.unwrap();
    assert_eq!(ended.reason, EndReason::HeartbeatLost);
    assert!(ended.checkpoint.is_none());
}

#[tokio::test]
async fn commit_failure_and_backpressure_preserve_last_durable_checkpoint() {
    for block in [false, true] {
        let journal = Arc::new(Journal::default());
        let (mut socket, _stop, task) = start(journal.clone(), None, 999).await;
        hello(&mut socket).await;
        ready(&mut socket).await;
        journal.wait(1).await;
        journal.block_messages.store(block, Ordering::SeqCst);
        journal.fail_messages.store(!block, Ordering::SeqCst);
        for sequence in 1..=if block { 8 } else { 1 } {
            message(&mut socket, sequence, &(100 + sequence).to_string()).await;
        }
        let ended = task.await.unwrap();
        assert_eq!(
            ended.reason,
            if block {
                EndReason::Backpressure
            } else {
                EndReason::CommitUnavailable
            }
        );
        assert_eq!(ended.checkpoint.unwrap().sequence, 0);
        assert!(journal.admitted.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn slow_admission_keeps_heartbeats_live_without_advancing_durable_sequence() {
    let journal = Arc::new(Journal::default());
    let (mut socket, stop, task) = start(journal.clone(), None, 999).await;
    hello(&mut socket).await;
    ready(&mut socket).await;
    journal.wait(1).await;
    journal.block_messages.store(true, Ordering::SeqCst);
    message(&mut socket, 1, "100").await;
    for _ in 0..2 {
        send(&mut socket, json!({"op":1,"d":null})).await;
        assert_eq!(read(&mut socket).await, json!({"op":1,"d":1}));
        send(&mut socket, json!({"op":11,"d":null})).await;
    }
    stop.send(true).unwrap();
    assert_eq!(task.await.unwrap().checkpoint.unwrap().sequence, 0);
    assert!(journal.admitted.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_session_never_silently_identifies_again() {
    for resumable in [false, true] {
        let journal = Arc::new(Journal::default());
        let (mut socket, _stop, task) = start(journal.clone(), None, 999).await;
        hello(&mut socket).await;
        ready(&mut socket).await;
        journal.wait(1).await;
        send(&mut socket, json!({"op":9,"d":resumable})).await;
        let ended = task.await.unwrap();
        assert_eq!(ended.reason, EndReason::InvalidSession);
        assert_eq!(ended.checkpoint.is_some(), resumable);
        assert!(ended.retry_after >= std::time::Duration::from_secs(5));
    }
}

#[tokio::test]
async fn oversized_wire_message_never_reaches_admission() {
    let journal = Arc::new(Journal::default());
    let (mut socket, _stop, task) = start(journal.clone(), None, 999).await;
    hello(&mut socket).await;
    ready(&mut socket).await;
    journal.wait(1).await;
    let _ = socket
        .send(Message::Text("x".repeat(FRAME_LIMIT + 1)))
        .await;
    let ended = task.await.unwrap();
    assert_eq!(ended.reason, EndReason::Transport);
    assert_eq!(ended.checkpoint.unwrap().sequence, 0);
    assert_eq!(journal.commits.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn invalid_session_requires_new_identify_and_privileged_intent_refusal_is_terminal() {
    for code in [4007, 4009, 4014, 4004] {
        let journal = Arc::new(Journal::default());
        let (mut socket, _stop, task) = start(journal.clone(), None, 999).await;
        hello(&mut socket).await;
        ready(&mut socket).await;
        journal.wait(1).await;
        socket
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: "".into(),
            })))
            .await
            .unwrap();
        let ended = task.await.unwrap();
        assert!(ended.checkpoint.is_none());
        assert_eq!(ended.retryable(), matches!(code, 4007 | 4009));
    }
}

#[tokio::test]
async fn invalid_ready_identity_and_host_never_commit() {
    for change in ["identity", "host"] {
        let journal = Arc::new(Journal::default());
        let (mut socket, _stop, task) = start(journal.clone(), None, 999).await;
        hello(&mut socket).await;
        let mut data = json!({"session_id":"s","resume_gateway_url":"wss://gateway.discord.gg/","user":{"id":"77","bot":true}});
        if change == "identity" {
            data["user"]["id"] = "78".into();
        } else {
            data["resume_gateway_url"] = "wss://private.internal/".into();
        }
        send(&mut socket, json!({"op":0,"s":0,"t":"READY","d":data})).await;
        let ended = task.await.unwrap();
        assert!(matches!(
            ended.reason,
            EndReason::Protocol | EndReason::AuthenticationFailed
        ));
        assert!(ended.checkpoint.is_none());
        assert!(journal.commits.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn server_requested_heartbeats_cannot_exceed_platform_send_limit() {
    let (mut socket, _stop, task) = start(Arc::new(Journal::default()), None, 999).await;
    hello(&mut socket).await;
    for _ in 0..109 {
        send(&mut socket, json!({"op":1,"d":null})).await;
        assert_eq!(read(&mut socket).await["op"], 1);
    }
    send(&mut socket, json!({"op":1,"d":null})).await;
    let ended = task.await.unwrap();
    assert_eq!(ended.reason, EndReason::RateLimited);
    assert_eq!(ended.retry_after, std::time::Duration::from_secs(60));
}

#[tokio::test]
async fn shutdown_stops_without_resetting_the_checkpoint() {
    let journal = Arc::new(Journal::default());
    let (mut socket, stop, task) = start(journal.clone(), None, 999).await;
    hello(&mut socket).await;
    ready(&mut socket).await;
    journal.wait(1).await;
    stop.send(true).unwrap();
    let ended = task.await.unwrap();
    assert_eq!(ended.reason, EndReason::Stopped);
    assert!(!ended.retryable());
    assert_eq!(ended.checkpoint.unwrap().sequence, 0);
}

#[test]
fn missing_message_content_intent_and_untrusted_resume_origins_are_refused() {
    assert!(matches!(
        GatewayClient::new(Secret::new("fixture"), "77".into(), 1 << 9),
        Err(GatewayError::MissingMessageContentIntent)
    ));
    for url in [
        "wss://evil.example/",
        "wss://gateway.discord.gg.evil.example/",
        "ws://gateway.discord.gg/",
        "wss://user@gateway.discord.gg/",
        "wss://gateway.discord.gg/private",
        "wss://gateway.discord.gg:444/",
    ] {
        assert!(endpoint(url).is_err());
    }
    assert_eq!(
        endpoint("wss://gateway.discord.gg/?compress=zlib-stream")
            .unwrap()
            .query(),
        Some("v=10&encoding=json")
    );
}
