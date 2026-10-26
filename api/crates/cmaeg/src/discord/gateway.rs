use super::protocol::*;
use cmaeg_core::channel::Secret;
use futures_util::{future::BoxFuture, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{collections::VecDeque, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
    time::{timeout, Instant},
};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    WebSocketStream,
};

const IO_TIMEOUT: Duration = Duration::from_secs(5);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const COMMIT_TIMEOUT: Duration = Duration::from_secs(15);
const SENDS_PER_MINUTE: usize = 110; // Below Discord's 120/60 s ceiling.

/// One real Discord Gateway connection. The native-source owner must supervise
/// reconnects, obey retry_after, and retain the last durable checkpoint.
/// No environment switch creates or activates this client today.
pub struct GatewayClient {
    token: Secret,
    bot_id: String,
    intents: u64,
}
impl GatewayClient {
    pub fn new(token: Secret, bot_id: String, intents: u64) -> Result<Self, GatewayError> {
        if intents & (1 << 15) == 0 {
            return Err(GatewayError::MissingMessageContentIntent);
        }
        if token.as_str().is_empty()
            || token.as_str().len() > 2048
            || bot_id.is_empty()
            || bot_id.len() > 32
            || !bot_id.bytes().all(|b| b.is_ascii_digit())
            || intents & ((1 << 9) | (1 << 12)) == 0
            || intents & !MESSAGE_INTENTS != 0
        {
            return Err(GatewayError::InvalidConfiguration);
        }
        Ok(Self {
            token,
            bot_id,
            intents,
        })
    }

    pub async fn connect(
        &self,
        resume: Option<Checkpoint>,
        journal: Arc<dyn NativeDiscordJournal>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<GatewayEnd, GatewayError> {
        if *shutdown.borrow() {
            return Ok(State::new(resume).end(EndReason::Stopped, false));
        }
        let url = endpoint(
            resume
                .as_ref()
                .map_or("wss://gateway.discord.gg/", |state| {
                    state.resume_url.as_str()
                }),
        )?;
        timeout(IO_TIMEOUT, journal.authorize_connection(resume.as_ref()))
            .await
            .map_err(|_| GatewayError::Unavailable)??;
        let socket = timeout(
            IO_TIMEOUT,
            connect_async_with_config(url.as_str(), Some(socket_config()), true),
        )
        .await
        .map_err(|_| GatewayError::Unavailable)?
        .map_err(|_| GatewayError::Unavailable)?
        .0;
        let jitter = u64::from_le_bytes(uuid::Uuid::new_v4().as_bytes()[..8].try_into().unwrap());
        Ok(self.run(socket, resume, journal, shutdown, jitter).await)
    }

    pub(super) async fn run<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        mut socket: WebSocketStream<S>,
        resume: Option<Checkpoint>,
        journal: Arc<dyn NativeDiscordJournal>,
        mut shutdown: watch::Receiver<bool>,
        jitter: u64,
    ) -> GatewayEnd {
        let mut state = State::new(resume);
        if *shutdown.borrow() {
            return state.end(EndReason::Stopped, false);
        }
        let hello = match timeout(HELLO_TIMEOUT, socket.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str::<Value>(&text).ok(),
            _ => None,
        };
        let interval = hello
            .as_ref()
            .filter(|hello| hello["op"] == 10)
            .and_then(|hello| hello["d"]["heartbeat_interval"].as_u64());
        let Some(interval) = interval.filter(|value| (1000..=120_000).contains(value)) else {
            return state.end(EndReason::Protocol, false);
        };
        let command = match &state.committed {
            Some(resume) => {
                json!({"op":6,"d":{"token":self.token.as_str(),"session_id":resume.session_id,"seq":resume.sequence}})
            }
            None => {
                json!({"op":2,"d":{"token":self.token.as_str(),"intents":self.intents,"properties":{"os":"linux","browser":"cma-event-gateway","device":"cma-event-gateway"}}})
            }
        };
        if let Err(reason) = state.send(&mut socket, command).await {
            return state.end(reason, false);
        }
        let interval = Duration::from_millis(interval);
        let mut heartbeat_at =
            Instant::now() + Duration::from_millis(jitter % interval.as_millis() as u64);
        let mut awaiting_ack = false;
        let mut queue = VecDeque::new();
        let mut pending: Option<BoxFuture<'static, Result<Checkpoint, GatewayError>>> = None;
        loop {
            if pending.is_none() {
                if let Some(event) = queue.pop_front() {
                    let next = match checkpoint(state.committed.as_ref(), &event, &self.bot_id) {
                        Ok(next) => next,
                        Err(GatewayError::Authority) => {
                            return state.end(EndReason::AuthenticationFailed, true)
                        }
                        Err(_) => return state.end(EndReason::Protocol, false),
                    };
                    let journal = journal.clone();
                    pending = Some(Box::pin(async move {
                        timeout(COMMIT_TIMEOUT, journal.admit(&next, &event))
                            .await
                            .map_err(|_| GatewayError::Unavailable)??;
                        Ok(next)
                    }));
                }
            }
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {return state.end(EndReason::Stopped,false)}
                },
                committed = async {pending.as_mut().unwrap().await}, if pending.is_some() => {
                    pending = None;
                    match committed {
                        Ok(checkpoint)=>state.committed=Some(checkpoint),
                        Err(GatewayError::Authority)=>return state.end(EndReason::AuthorityRevoked,false),
                        Err(_)=>return state.end(EndReason::CommitUnavailable,false),
                    }
                },
                _ = tokio::time::sleep_until(heartbeat_at) => {
                    if awaiting_ack {return state.end(EndReason::HeartbeatLost,false)}
                    if let Err(reason)=state.send(&mut socket,json!({"op":1,"d":state.received})).await {return state.end(reason,false)}
                    awaiting_ack=true;
                    heartbeat_at=Instant::now()+interval;
                },
                frame=socket.next()=> {
                    let text=match frame {
                        Some(Ok(Message::Text(text)))=>text,
                        Some(Ok(Message::Close(frame)))=> {
                            let (reason,clear)=frame.map_or((EndReason::Transport,false),|frame|close(frame.code.into()));
                            return state.end(reason,clear)
                        },
                        Some(Ok(Message::Ping(_)|Message::Pong(_)))=>continue,
                        Some(Ok(_))=>return state.end(EndReason::Protocol,false),
                        _=>return state.end(EndReason::Transport,false),
                    };
                    let value:Value=match serde_json::from_str(&text) {Ok(value)=>value,Err(_)=>return state.end(EndReason::Protocol,false)};
                    match value["op"].as_u64() {
                        Some(0)=> {
                            let Some(sequence)=value["s"].as_u64() else {return state.end(EndReason::Protocol,false)};
                            if state.received.is_some_and(|received|sequence<=received) {continue}
                            let Some(name)=value["t"].as_str().filter(|name|!name.is_empty()&&name.len()<=80) else {return state.end(EndReason::Protocol,false)};
                            if !value["d"].is_object() {return state.end(EndReason::Protocol,false)}
                            if name=="MESSAGE_CREATE" && snowflake(&value["d"]["id"]).is_none() {return state.end(EndReason::Protocol,false)}
                            if queue.len()>=QUEUE_LIMIT {return state.end(EndReason::Backpressure,false)}
                            state.received=Some(sequence);
                            queue.push_back(Dispatch {name:name.into(),sequence,data:value["d"].clone()});
                        },
                        Some(1)=> {
                            if let Err(reason)=state.send(&mut socket,json!({"op":1,"d":state.received})).await {return state.end(reason,false)}
                            awaiting_ack=true;
                        },
                        Some(7)=>return state.end(EndReason::Reconnect,false),
                        Some(9)=> {
                            let Some(resumable)=value["d"].as_bool() else {return state.end(EndReason::Protocol,false)};
                            return state.end(EndReason::InvalidSession,!resumable)
                        },
                        Some(11)=>awaiting_ack=false,
                        _=>return state.end(EndReason::Protocol,false),
                    }
                }
            }
        }
    }
}
pub(super) fn socket_config() -> WebSocketConfig {
    WebSocketConfig {
        max_message_size: Some(FRAME_LIMIT),
        max_frame_size: Some(FRAME_LIMIT),
        write_buffer_size: 16 * 1024,
        max_write_buffer_size: 64 * 1024,
        ..Default::default()
    }
}
struct State {
    committed: Option<Checkpoint>,
    received: Option<u64>,
    sent: VecDeque<Instant>,
}
impl State {
    fn new(committed: Option<Checkpoint>) -> Self {
        Self {
            received: committed.as_ref().map(|state| state.sequence),
            committed,
            sent: VecDeque::new(),
        }
    }
    fn end(self, reason: EndReason, clear: bool) -> GatewayEnd {
        GatewayEnd {
            reason,
            checkpoint: if clear { None } else { self.committed },
            retry_after: if reason == EndReason::RateLimited {
                Duration::from_secs(60)
            } else {
                Duration::from_secs(5)
            },
        }
    }
    async fn send<S: AsyncRead + AsyncWrite + Unpin>(
        &mut self,
        socket: &mut WebSocketStream<S>,
        body: Value,
    ) -> Result<(), EndReason> {
        let now = Instant::now();
        while self
            .sent
            .front()
            .is_some_and(|time| now.duration_since(*time) >= Duration::from_secs(60))
        {
            self.sent.pop_front();
        }
        if self.sent.len() >= SENDS_PER_MINUTE {
            return Err(EndReason::RateLimited);
        }
        let body = body.to_string();
        if body.len() > 4096 {
            return Err(EndReason::Protocol);
        }
        self.sent.push_back(now);
        timeout(IO_TIMEOUT, socket.send(Message::Text(body)))
            .await
            .map_err(|_| EndReason::Transport)?
            .map_err(|_| EndReason::Transport)
    }
}
