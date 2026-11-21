use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use futures_util::SinkExt as _;
use tokio::process::Command;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::Message;

use super::*;

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_pending_backoff() {
    let (stop, signal) = watch::channel(false);
    let retry = tokio::spawn(wait_to_retry(signal, Duration::from_secs(8)));
    tokio::time::advance(Duration::from_secs(7)).await;
    assert!(!retry.is_finished());
    stop.send(true).unwrap();
    assert!(!retry.await.unwrap());
}

#[tokio::test]
async fn an_exited_desktop_rotates_the_token_and_a_live_replacement_is_drained_on_shutdown() {
    let trigger = std::env::temp_dir().join(format!("cma-desktop-exit-{}", uuid::Uuid::new_v4()));
    let initial = fixture_process(
        r#"while [ ! -f "$1" ]; do sleep .02; done; printf '\001\002'"#,
        &trigger,
    );
    let first_token = initial.websocket_token().to_owned();
    let (stop, signal) = watch::channel(false);
    let (state, mut current) = watch::channel(State::Starting);
    let launches = Arc::new(AtomicUsize::new(0));
    let count = launches.clone();
    let token = Arc::new(StdMutex::new(String::new()));
    let expected = token.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let display = format!("ws://{}/display", listener.local_addr().unwrap());
    let service = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_hdr_async(socket, ExpectToken(expected))
            .await
            .unwrap();
        socket
            .send(Message::Binary(b"RFB 003.008\n".to_vec()))
            .await
            .unwrap();
    });
    let task = tokio::spawn(async move {
        let mut driver = Recovery {
            state,
            shutdown: signal,
            budget: RestartBudget::new(),
        };
        driver
            .run(
                initial,
                move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    let child =
                        fixture_process(r#"cat >/dev/null; printf '\001\002'"#, Path::new("."));
                    *token.lock().unwrap() = child.websocket_token().to_owned();
                    Ok(child)
                },
                &display,
            )
            .await;
    });
    let old = ready_generation(&mut current).await;
    assert_eq!(old.token(), first_token);
    std::fs::write(&trigger, b"").unwrap();
    tokio::time::timeout(Duration::from_secs(2), old.invalidated())
        .await
        .unwrap();
    let replacement = ready_generation(&mut current).await;
    assert_ne!(replacement.token(), first_token);
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    assert!(replacement.active());
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap();
    assert!(!replacement.active());
    assert_eq!(launches.load(Ordering::SeqCst), 1);
    service.await.unwrap();
    std::fs::remove_file(trigger).unwrap();
}

#[tokio::test]
async fn an_unproved_cleanup_fences_the_desktop_without_launching_a_replacement() {
    let initial = fixture_process(r#"printf '\001'"#, Path::new("."));
    let (stop, signal) = watch::channel(false);
    let (state, mut current) = watch::channel(State::Starting);
    let task = tokio::spawn(async move {
        let mut driver = Recovery {
            state,
            shutdown: signal,
            budget: RestartBudget::new(),
        };
        driver
            .run(
                initial,
                || panic!("unproved cleanup cannot authorize launch"),
                DISPLAY_URL,
            )
            .await;
    });
    tokio::time::timeout(
        Duration::from_secs(2),
        current.wait_for(|state| matches!(state, State::Failed)),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(!task.is_finished(), "failed generation retains its leader");
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
}

struct ExpectToken(Arc<StdMutex<String>>);

impl Callback for ExpectToken {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        let token = self.0.lock().unwrap();
        if request
            .headers()
            .get("host")
            .is_some_and(|value| value == token.as_str())
        {
            Ok(response)
        } else {
            Err(axum::http::Response::builder()
                .status(403)
                .body(None)
                .unwrap())
        }
    }
}

fn fixture_process(script: &str, argument: &Path) -> DesktopProcess {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", script, "fixture"])
        .arg(argument)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    DesktopProcess::spawn_in_group(command, true).unwrap()
}

async fn ready_generation(current: &mut watch::Receiver<State>) -> DesktopGeneration {
    let state = tokio::time::timeout(
        Duration::from_secs(4),
        current.wait_for(|state| matches!(state, State::Ready(generation) if generation.active())),
    )
    .await
    .unwrap()
    .unwrap();
    match &*state {
        State::Ready(generation) => generation.clone(),
        _ => unreachable!("readiness wait checked the state"),
    }
}
