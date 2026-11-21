use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::AsyncReadExt as _;
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::desktop_process::transport::{connect, DISPLAY_URL};
use crate::desktop_process::GenerationDirectory;

const LISTENER_FIXTURE: &str = "desktop_process::supervision::tests::owned_display_fixture";

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
    let root = TestDirectory::new();
    let trigger = root.0.join("exit");
    let release = root.0.join("replacement-ready");
    let first_ready = root.0.join("initial-ready");
    std::fs::write(&first_ready, b"").unwrap();
    let initial = serving_process(&first_ready, Some(&trigger));
    let first_token = initial.websocket_token().to_owned();
    let (stop, signal) = watch::channel(false);
    let (state, mut current) = watch::channel(State::Starting);
    let launches = Arc::new(AtomicUsize::new(0));
    let count = launches.clone();
    let second_ready = release.clone();
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
                    Ok(serving_process(&second_ready, None))
                },
                None,
            )
            .await;
    });
    let old = ready_generation(&mut current).await;
    assert_eq!(old.token(), first_token);
    std::fs::write(&trigger, b"").unwrap();
    tokio::time::timeout(Duration::from_secs(2), old.invalidated())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while launches.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(250),
        current.wait_for(|state| matches!(state, State::Ready(_) | State::Failed),)
    )
    .await
    .is_err());
    assert!(first_ready.exists());
    std::fs::write(release, b"").unwrap();
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
}

#[tokio::test]
async fn exit_before_initial_readiness_does_not_start_recovery() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", r#"printf '\001\002'"#]);
    let (first_ready, confirmed) = oneshot::channel();
    let (state, current) = watch::channel(State::Starting);
    let (_stop, shutdown) = watch::channel(false);
    let mut driver = Recovery {
        state,
        shutdown,
        budget: RestartBudget::new(),
    };
    tokio::time::timeout(
        Duration::from_secs(2),
        driver.run(
            fixture_process(command),
            || panic!("exit before initial readiness cannot authorize launch"),
            Some(first_ready),
        ),
    )
    .await
    .unwrap();
    assert!(confirmed.await.is_err());
    assert!(matches!(*current.borrow(), State::Failed));
}

#[tokio::test]
async fn an_unproved_cleanup_fences_the_desktop_without_launching_a_replacement() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", r#"printf '\001'"#]);
    let initial = fixture_process(command);
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
                None,
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

#[tokio::test]
async fn unrelated_unix_listeners_never_receive_a_desktop_credential() {
    let root = TestDirectory::new();
    let release = root.0.join("ready");
    std::fs::write(&release, b"").unwrap();
    let mut child = serving_process(&release, None);
    let (path, leader) = child.display_socket();
    let held = path.with_extension("held");
    let token = child.websocket_token().to_owned();
    let listener = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(listener) = authenticate(&token, &path, leader).await {
                return listener;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (generation, _owner) = DesktopGeneration::supervised(token.clone(), listener);
    let mut real = connect(&generation, DISPLAY_URL).await.unwrap();
    assert_eq!(
        real.next().await.unwrap().unwrap(),
        Message::Binary(b"RFB 003.008\n".to_vec())
    );
    std::fs::rename(&path, &held).unwrap();
    let foreign = UnixListener::bind(&path).unwrap();
    assert!(authenticate(&token, &path, leader).await.is_err());
    let (mut socket, _) = foreign.accept().await.unwrap();
    assert_eq!(socket.read(&mut [0; 1]).await.unwrap(), 0);
    assert!(connect(&generation, DISPLAY_URL).await.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), foreign.accept())
            .await
            .is_err()
    );
    std::fs::remove_file(held).unwrap();
    child.finish(false).await.unwrap();
}

#[tokio::test]
#[ignore = "subprocess fixture; requires a private generation directory"]
async fn owned_display_fixture() {
    let dir = PathBuf::from(std::env::var_os("CMA_DESKTOP_GENERATION_DIR").unwrap());
    let ready = PathBuf::from(std::env::var_os("CMA_TEST_DESKTOP_READY").unwrap());
    while !ready.is_file() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let token = std::env::var("CMA_DESKTOP_TOKEN").unwrap();
    let listener = UnixListener::bind(dir.join("display.sock")).unwrap();
    loop {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::spawn(serve(socket, token.clone()));
    }
}

async fn serve(socket: UnixStream, token: String) {
    if let Ok(mut websocket) = tokio_tungstenite::accept_hdr_async(socket, ExpectToken(token)).await
    {
        let _ = websocket
            .send(Message::Binary(b"RFB 003.008\n".to_vec()))
            .await;
    }
}

struct ExpectToken(String);

impl Callback for ExpectToken {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        if request
            .headers()
            .get("host")
            .is_some_and(|value| value == self.0.as_str())
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

fn serving_process(ready: &Path, exit: Option<&Path>) -> DesktopProcess {
    let mut command = Command::new("/bin/bash");
    command
        .args([
            "-c",
            r#"
            "$1" --exact "$2" --ignored >&2 &
            peer=$!
            if [ -z "$3" ]; then
                cat >/dev/null
            else
                while [ ! -f "$3" ]; do
                    read -r -t .02 ignored
                    [ "$?" -gt 128 ] || break
                done
            fi
            kill "$peer"
            wait "$peer" 2>/dev/null || true
            printf '\001\002'
        "#,
            "fixture",
        ])
        .arg(std::env::current_exe().unwrap())
        .arg(LISTENER_FIXTURE)
        .arg(exit.unwrap_or_else(|| Path::new("")))
        .env("CMA_TEST_DESKTOP_READY", ready);
    fixture_process(command)
}

fn fixture_process(mut command: Command) -> DesktopProcess {
    let files = GenerationDirectory::create().unwrap();
    command
        .env("CMA_DESKTOP_GENERATION_DIR", &files.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = DesktopProcess::spawn_in_group(command, true).unwrap();
    child.guard_files = Some(files);
    child
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

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("cma-desktop-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
