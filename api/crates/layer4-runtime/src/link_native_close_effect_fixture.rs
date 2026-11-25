use super::super::{echo_supervisor, router, Supervisor};
use axum::body::{to_bytes, Body};
use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::Response;
use layer3_adapter::owner_protocol::{TransportOwner, OWNER_HEADER};
use layer3_adapter::wire::RequestFrame;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};

const AGENT: &str = "agent-1";

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        for path in [
            self.0.clone(),
            self.0.with_extension("checkpoints"),
            self.0.with_extension("runtime-state"),
        ] {
            match std::fs::remove_dir_all(path) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => panic!("removing runtime fixture: {error}"),
            }
        }
    }
}

struct Observations {
    supervisor: Arc<Supervisor>,
    bridge: Arc<crate::bridge::Bridge>,
    ordinary_peer: OnceLock<SocketAddr>,
    close_peer: OnceLock<SocketAddr>,
    events: mpsc::UnboundedSender<String>,
    release: Notify,
}

fn event(stage: &str) -> String {
    let micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros();
    format!("{stage} {micros}\n")
}

async fn observe(
    State(observations): State<Arc<Observations>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if request.uri().path() != "/runtime/control" {
        return next.run(request).await;
    }
    let (parts, body) = request.into_parts();
    let bytes = to_bytes(body, 4096).await.unwrap();
    let frame: RequestFrame = serde_json::from_slice(&bytes).unwrap();
    let request = Request::from_parts(parts, Body::from(bytes));
    match frame {
        RequestFrame::Resources => {
            observations
                .ordinary_peer
                .set(peer)
                .expect("one ordinary request");
            let response = next.run(request).await;
            assert!(response.status().is_success());
            assert!(!observations.bridge.is_ended());
            observations.events.send(event("held")).unwrap();
            observations.release.notified().await;
            response
        }
        RequestFrame::CloseAgent(frame) => {
            assert_ne!(*observations.ordinary_peer.get().unwrap(), peer);
            observations
                .close_peer
                .set(peer)
                .expect("one CloseAgent wire request");
            let owner =
                TransportOwner::from_header(request.headers()[OWNER_HEADER].to_str().unwrap())
                    .unwrap();
            assert_eq!(owner.agent_id, AGENT);
            assert_eq!(owner.epoch, frame.authority_fence);
            assert!(!observations.bridge.is_ended());
            observations.events.send(event("wire")).unwrap();
            let response = next.run(request).await;
            if !observations.bridge.is_ended()
                || observations.supervisor.bridge(AGENT).await.is_some()
            {
                let bytes = to_bytes(response.into_body(), 4096).await.unwrap();
                let response: layer3_adapter::wire::ResponseFrame =
                    serde_json::from_slice(&bytes).unwrap();
                panic!("CloseAgent left the runtime process alive: {response:?}");
            }
            observations.events.send(event("effect")).unwrap();
            response
        }
        other => panic!("unexpected runtime control frame: {other:?}"),
    }
}

async fn exchange_observations() {
    let target = std::env::var("CMA_NATIVE_CLOSE_COORDINATION").unwrap();
    let target: SocketAddr = target.parse().unwrap();
    assert!(target.ip().is_loopback());
    let mut stream = BufReader::new(TcpStream::connect(target).await.unwrap());
    let (home, supervisor) = echo_supervisor("native-close-effect").await;
    let _home = Home(home);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    stream
        .get_mut()
        .write_all(format!("{}\n", listener.local_addr().unwrap()).as_bytes())
        .await
        .unwrap();
    let mut token = String::new();
    assert_ne!(stream.read_line(&mut token).await.unwrap(), 0);
    let authentication =
        crate::control_auth::ControlAuth::from_token(token.trim_end().to_owned()).unwrap();
    let (events, mut event_stream) = mpsc::unbounded_channel();
    let observations = Arc::new(Observations {
        bridge: supervisor.bridge(AGENT).await.unwrap(),
        supervisor: Arc::clone(&supervisor),
        ordinary_peer: OnceLock::new(),
        close_peer: OnceLock::new(),
        release: Notify::new(),
        events,
    });
    let app = router(supervisor)
        .layer(from_fn_with_state(observations.clone(), observe))
        .layer(from_fn_with_state(
            authentication,
            crate::control_auth::authenticate,
        ));
    let runtime = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    );
    let protocol = async {
        for expected in ["held", "wire", "effect"] {
            let event = event_stream.recv().await.unwrap();
            assert_eq!(event.split_whitespace().next(), Some(expected));
            stream.get_mut().write_all(event.as_bytes()).await.unwrap();
        }
        let mut command = String::new();
        stream.read_line(&mut command).await.unwrap();
        assert_eq!(command, "release\n");
        observations.release.notify_one();
        command.clear();
        stream.read_line(&mut command).await.unwrap();
        assert_eq!(command, "verify\n");
        assert!(observations.bridge.is_ended());
        assert!(observations.supervisor.bridge(AGENT).await.is_none());
        stream
            .get_mut()
            .write_all(event("closed").as_bytes())
            .await
            .unwrap();
    };
    tokio::select! {
        result = runtime => panic!("runtime fixture stopped: {result:?}"),
        () = protocol => {}
    }
}

#[tokio::test]
#[ignore = "receiver subprocess for the Layer 3 native close effect control"]
async fn runtime_receiver() {
    tokio::time::timeout(Duration::from_secs(15), exchange_observations())
        .await
        .expect("bounded close effect receiver");
}
