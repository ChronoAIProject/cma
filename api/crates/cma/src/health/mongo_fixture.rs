//! Loopback Mongo wire peer: the locked driver owns the real pool and sockets.
//! Only hello and ping are needed; no database, credentials or external service.
use std::{
    io,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use mongodb::{
    bson::{doc, Document},
    event::{cmap::CmapEvent, EventHandler},
    options::ClientOptions,
    Client,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::watch,
    task::{JoinHandle, JoinSet},
};

use super::{ReadinessProbe, StoreError};

pub(super) struct DriverProbe(pub Client);

#[async_trait::async_trait]
impl ReadinessProbe for DriverProbe {
    async fn ping(&self) -> Result<(), StoreError> {
        self.0
            .database("readiness")
            .run_command(doc! { "ping": 1 })
            .await
            .map(|_| ())
            .map_err(|_| StoreError::Unavailable("test Mongo ping".into()))
    }
}

#[derive(Default)]
pub(super) struct Trace {
    pub checkout_started: usize,
    pub checked_out: usize,
    pub checked_in: usize,
    pub dropped: usize,
}

pub(super) struct MongoPeer {
    pub client: Client,
    pub trace: Arc<Mutex<Trace>>,
    pub pings: Arc<AtomicUsize>,
    pub peer_closed: Arc<AtomicUsize>,
    hold: watch::Sender<bool>,
    server: JoinHandle<()>,
}

impl MongoPeer {
    pub async fn start() -> Self {
        Self::start_with_diagnostics(None).await
    }

    pub async fn start_with_diagnostics(
        recorder: Option<Arc<crate::readiness_diagnostics::Recorder>>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = listener.local_addr().unwrap();
        let (hold, blocking) = watch::channel(false);
        let pings = Arc::new(AtomicUsize::new(0));
        let peer_closed = Arc::new(AtomicUsize::new(0));
        let observed = pings.clone();
        let closed = peer_closed.clone();
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.unwrap();
                        connections.spawn(serve(socket, blocking.clone(), observed.clone(), closed.clone()));
                    }
                    Some(result) = connections.join_next() => {
                        result.unwrap().unwrap();
                    }
                }
            }
        });
        let trace = Arc::new(Mutex::new(Trace::default()));
        let events = trace.clone();
        let mut options =
            ClientOptions::parse(format!("mongodb://{endpoint}/?directConnection=true"))
                .await
                .unwrap();
        // Test-only saturation, with the production driver's pool implementation.
        options.max_pool_size = Some(1);
        options.server_selection_timeout = Some(Duration::from_secs(10));
        let diagnostics = recorder.map(|r| crate::store::attach_readiness_fixture(&mut options, r));
        options.cmap_event_handler = Some(EventHandler::callback(move |event| {
            if let Some(diagnostics) = &diagnostics {
                diagnostics(&event);
            }
            let mut trace = events.lock().unwrap();
            match event {
                CmapEvent::ConnectionCheckoutStarted(_) => trace.checkout_started += 1,
                CmapEvent::ConnectionCheckedOut(_) => trace.checked_out += 1,
                CmapEvent::ConnectionCheckedIn(_) => trace.checked_in += 1,
                CmapEvent::ConnectionClosed(event)
                    if event.reason == mongodb::event::cmap::ConnectionClosedReason::Dropped =>
                {
                    trace.dropped += 1
                }
                _ => (),
            }
        }));
        let client = Client::with_options(options).unwrap();
        tokio::time::timeout(Duration::from_secs(3), DriverProbe(client.clone()).ping())
            .await
            .unwrap()
            .unwrap();
        let peer = Self {
            client,
            trace,
            pings,
            peer_closed,
            hold,
            server,
        };
        peer.wait_for(|| peer.in_use() == 0).await;
        peer
    }

    pub fn block(&self, value: bool) {
        self.hold.send_replace(value);
    }

    pub fn ping_count(&self) -> usize {
        self.pings.load(Ordering::SeqCst)
    }

    pub fn in_use(&self) -> usize {
        let trace = self.trace.lock().unwrap();
        trace.checked_out - trace.checked_in
    }

    pub async fn wait_for(&self, condition: impl Fn() -> bool) {
        tokio::time::timeout(Duration::from_secs(3), async {
            while !condition() {
                assert!(!self.server.is_finished(), "Mongo peer exited");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("driver condition did not become true");
    }
}

impl Drop for MongoPeer {
    fn drop(&mut self) {
        // Dropping the server's JoinSet also cancels all accepted sockets.
        self.server.abort();
    }
}

async fn serve(
    mut socket: TcpStream,
    mut hold: watch::Receiver<bool>,
    pings: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
) -> io::Result<()> {
    loop {
        let mut header = [0_u8; 16];
        match socket.read_exact(&mut header).await {
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        }
        let length = i32::from_le_bytes(header[0..4].try_into().unwrap());
        assert!((21..65536).contains(&length));
        assert_eq!(i32::from_le_bytes(header[12..16].try_into().unwrap()), 2013);
        let mut body = vec![0; length as usize - 16];
        socket.read_exact(&mut body).await?;
        assert_eq!(body[4], 0, "one BSON body section");
        let command = Document::from_reader(&mut &body[5..]).unwrap();
        let reply = if command.contains_key("hello") || command.contains_key("isMaster") {
            doc! { "ok": 1, "isWritablePrimary": true, "ismaster": true,
            "minWireVersion": 0, "maxWireVersion": 25,
            "maxBsonObjectSize": 16777216, "maxMessageSizeBytes": 48000000,
            "maxWriteBatchSize": 100000, "logicalSessionTimeoutMinutes": 30 }
        } else {
            if command.contains_key("ping") {
                pings.fetch_add(1, Ordering::SeqCst);
                while *hold.borrow() {
                    tokio::select! {
                        changed = hold.changed() => { if changed.is_err() { return Ok(()); } }
                        read = socket.read_u8() => {
                            assert!(read.is_err(), "no further command before ping response");
                            closed.fetch_add(1, Ordering::SeqCst);
                            return Ok(());
                        }
                    }
                }
            } else {
                assert!(
                    command.contains_key("endSessions"),
                    "unexpected command: {command:?}"
                );
            }
            doc! { "ok": 1 }
        };
        let bson = mongodb::bson::serialize_to_vec(&reply).unwrap();
        let mut reply = Vec::new();
        reply.extend_from_slice(&(21_i32 + bson.len() as i32).to_le_bytes());
        reply.extend_from_slice(&1_i32.to_le_bytes());
        reply.extend_from_slice(&header[4..8]);
        reply.extend_from_slice(&2013_i32.to_le_bytes());
        reply.extend_from_slice(&[0; 5]);
        reply.extend_from_slice(&bson);
        socket.write_all(&reply).await?;
    }
}
