use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;
use tokio_tungstenite::tungstenite::handshake::server::{
    Callback, ErrorResponse, Request, Response,
};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::desktop_process::transport::{connect, DISPLAY_URL};
use crate::desktop_process::GenerationDirectory;

mod startup_limit;

const SHORT_STARTUP_LIMIT: Duration = Duration::from_secs(30);
const LISTENER_FIXTURE: &str = "desktop_process::supervision::tests::owned_display_fixture";

#[tokio::test]
async fn omarchy_waits_for_the_current_launchers_readiness_marker() {
    let output = tokio::time::timeout(
        Duration::from_secs(20),
        Command::new("python3")
            .args([
                "-B",
                "-c",
                include_str!("../../omarchy/session_tests.py"),
                include_str!("../../omarchy/session.sh"),
                include_str!("../../omarchy/inner-session.sh"),
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

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
    let initial = serving_process(&first_ready, Some(&trigger), false);
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
            startup_limit: SHORT_STARTUP_LIMIT,
        };
        driver
            .run(
                initial,
                move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Ok(serving_process(&second_ready, None, false))
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
async fn repeated_replacement_failures_leave_the_desktop_terminal() {
    let root = TestDirectory::new();
    let ready = root.0.join("ready");
    let exit = root.0.join("exit");
    std::fs::write(&ready, b"").unwrap();
    let initial = serving_process(&ready, Some(&exit), false);
    let launcher_pid = initial.child.id().unwrap() as i32;
    let (_stop, shutdown) = watch::channel(false);
    let (state, mut current) = watch::channel(State::Starting);
    let (first_attempt, launched) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut recovery = Recovery {
            state,
            shutdown,
            budget: RestartBudget::new(),
            startup_limit: SHORT_STARTUP_LIMIT,
        };
        let mut launches = 0;
        let mut first_attempt = Some(first_attempt);
        recovery
            .run(
                initial,
                || {
                    launches += 1;
                    assert_eq!(unsafe { libc::kill(launcher_pid, 0) }, -1);
                    assert_eq!(
                        std::io::Error::last_os_error().raw_os_error(),
                        Some(libc::ESRCH)
                    );
                    if let Some(first_attempt) = first_attempt.take() {
                        first_attempt.send(()).unwrap();
                    }
                    Err(crate::failure::RuntimeError::unavailable(
                        "desktop_start_failed",
                        "fixture launch failure",
                    ))
                },
                None,
            )
            .await;
        launches
    });
    let generation = ready_generation(&mut current).await;
    std::fs::write(exit, b"").unwrap();
    tokio::time::timeout(Duration::from_secs(2), generation.invalidated())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), launched)
        .await
        .unwrap()
        .unwrap();
    tokio::time::pause();
    let launches = tokio::time::timeout(Duration::from_secs(20), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(launches, 4);
    assert!(matches!(*current.borrow(), State::Failed));
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
        startup_limit: SHORT_STARTUP_LIMIT,
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
            startup_limit: SHORT_STARTUP_LIMIT,
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
    let mut child = serving_process(&release, None, false);
    let (files, leader) = child.display_directory();
    let path = files.path.join("display.sock");
    let held = path.with_extension("held");
    let token = child.websocket_token().to_owned();
    let mut backend = None;
    let listener = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(listener) =
                authenticate(&token, &files.path, leader, &files.token_map, &mut backend).await
            {
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
    assert!(
        authenticate(&token, &files.path, leader, &files.token_map, &mut backend)
            .await
            .is_err()
    );
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
async fn the_owned_proxy_cannot_publish_or_relay_a_foreign_backend() {
    let root = TestDirectory::new();
    let release = root.0.join("ready");
    std::fs::write(&release, b"").unwrap();

    let mut isolated_proxy = serving_process(&release, None, true);
    let (files, leader) = isolated_proxy.display_directory();
    let backend_path = files.path.join("rfb.sock");
    let foreign = UnixListener::bind(&backend_path).unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !files.path.join("display.sock").exists() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let token = isolated_proxy.websocket_token();
    let mut backend = None;
    assert!(
        authenticate(token, &files.path, leader, &files.token_map, &mut backend)
            .await
            .is_err()
    );
    assert!(backend.is_none());
    assert!(!files.path.join("token-ready").exists());
    assert_eq!(files.token_map.metadata().unwrap().len(), 0);
    let (mut rejected, _) = foreign.accept().await.unwrap();
    assert_eq!(rejected.read(&mut [0; 1]).await.unwrap(), 0);
    isolated_proxy.finish(false).await.unwrap();

    let mut child = serving_process(&release, None, false);
    let (files, leader) = child.display_directory();
    let token = child.websocket_token().to_owned();
    let binding = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(binding) =
                authenticate(&token, &files.path, leader, &files.token_map, &mut backend).await
            {
                return binding;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    let (generation, _owner) = DesktopGeneration::supervised(token, binding);
    let path = files.path.join("rfb.sock");
    let saved = path.with_extension("held");
    let mut viewer = connect(&generation, DISPLAY_URL).await.unwrap();
    assert_eq!(
        viewer.next().await.unwrap().unwrap(),
        Message::Binary(b"RFB 003.008\n".to_vec())
    );
    std::fs::rename(&path, &saved).unwrap();
    let foreign = UnixListener::bind(&path).unwrap();
    assert!(connect(&generation, DISPLAY_URL).await.is_err());
    assert!(
        tokio::time::timeout(Duration::from_millis(100), foreign.accept())
            .await
            .is_err()
    );
    std::fs::remove_file(saved).unwrap();
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
    if std::env::var_os("CMA_TEST_PROXY_ONLY").is_none() {
        let rfb = UnixListener::bind(dir.join("rfb.sock")).unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = rfb.accept().await {
                let _ = socket.write_all(b"RFB 003.008\n").await;
            }
        });
    }
    let mapping = PathBuf::from(std::env::var_os("CMA_DESKTOP_TOKEN_FILE").unwrap());
    let listener = UnixListener::bind(dir.join("display.sock")).unwrap();
    loop {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::spawn(serve(socket, token.clone(), mapping.clone()));
    }
}

async fn serve(socket: UnixStream, token: String, mapping: PathBuf) {
    if let Ok(mut websocket) = tokio_tungstenite::accept_hdr_async(socket, ExpectToken(token)).await
    {
        let Ok(mapping) = std::fs::read_to_string(mapping) else {
            return;
        };
        let (_, endpoint) = mapping.trim_end().split_once(": unix_socket:").unwrap();
        let mut backend = UnixStream::connect(endpoint).await.unwrap();
        let mut greeting = [0; 12];
        backend.read_exact(&mut greeting).await.unwrap();
        let _ = websocket.send(Message::Binary(greeting.to_vec())).await;
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

fn serving_process(ready: &Path, exit: Option<&Path>, proxy_only: bool) -> DesktopProcess {
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
    if proxy_only {
        command.env("CMA_TEST_PROXY_ONLY", "1");
    }
    fixture_process(command)
}

fn fixture_process(mut command: Command) -> DesktopProcess {
    let files = GenerationDirectory::create().unwrap();
    command
        .env("CMA_DESKTOP_GENERATION_DIR", &files.path)
        .env(
            "CMA_DESKTOP_TOKEN_FILE",
            super::super::transport::token_map::endpoint(&files.token_map),
        )
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
