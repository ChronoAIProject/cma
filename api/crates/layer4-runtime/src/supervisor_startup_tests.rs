use std::collections::BTreeMap;
use std::future::{pending, ready};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use layer3_adapter::desktop_ownership::wire::{DESKTOP_STATE_FAILED, DESKTOP_STATE_HEADER};
use layer3_adapter::{wire::RequestFrame, wire::ResponseFrame, RuntimeStatusRequest};
use tokio::time::Instant;

use super::*;
use crate::desktop_process::{legacy::LegacyDesktop, DesktopSession, DesktopState};

struct TestRoot(PathBuf);

impl Drop for TestRoot {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove desktop startup fixture");
    }
}

fn startup(machine: bool, extended: bool) -> (TestRoot, Arc<Supervisor>) {
    let path = std::env::temp_dir()
        .canonicalize()
        .unwrap()
        .join(format!("cma-desktop-startup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    let root = TestRoot(path);
    let mut config = RuntimeConfig::for_test(root.0.join("home"));
    std::fs::create_dir_all(&config.home).unwrap();
    std::fs::create_dir_all(&config.state_root).unwrap();
    config.extended_desktop_startup = extended;
    config.home_identity = Some("a".repeat(43));
    let mut supervisor = Supervisor::new(config, SupervisorBoundary::default()).unwrap();
    // The tests supply the readiness probe and, where needed, a managed child.
    let inner = Arc::get_mut(&mut supervisor).unwrap();
    inner.config.machine_included = machine;
    if machine {
        inner.desktop_process = Some(DesktopSession::Legacy(Box::new(LegacyDesktop::probe_only(
            "desktop-startup-token".into(),
        ))));
    }
    (root, supervisor)
}

fn assert_desktop_failed(supervisor: &Supervisor) {
    assert_eq!(supervisor.desktop_state(), Some(DesktopState::Failed));
    let Err(unavailable) = supervisor
        .desktop_generation()
        .expect("machine runtime exposes desktop state")
    else {
        panic!("failed desktop cannot expose an active generation");
    };
    assert!(!unavailable.retryable);
}

#[test]
fn selects_the_extended_budget_only_with_a_valid_opt_in() {
    const FLAG: &str = "CHRONO_RUNTIME_DESKTOP_EXTENDED_STARTUP_ENABLED";
    let base = BTreeMap::from([("CHRONO_SANDBOX_ID".into(), "startup-fixture".into())]);
    assert!(
        !RuntimeConfig::from_values(base.clone())
            .unwrap()
            .extended_desktop_startup
    );
    for (raw, enabled) in [("0", false), ("false", false), ("1", true), ("true", true)] {
        let mut values = base.clone();
        values.insert(FLAG.into(), raw.into());
        assert_eq!(
            RuntimeConfig::from_values(values)
                .unwrap()
                .extended_desktop_startup,
            enabled
        );
    }
    for invalid in ["", "yes", "TRUE", " true"] {
        let mut values = base.clone();
        values.insert(FLAG.into(), invalid.into());
        assert_eq!(
            RuntimeConfig::from_values(values).unwrap_err(),
            format!("{FLAG} must be 0, 1, false or true")
        );
    }
}

#[tokio::test(start_paused = true)]
async fn machine_disabled_starts_without_a_desktop_or_probe() {
    let (_root, supervisor) = startup(false, true);
    assert!(supervisor.desktop_process.is_none());
    let supervisor =
        Supervisor::finish_startup_with_probe(supervisor, || -> std::future::Ready<bool> {
            panic!("a non-Machine runtime attempted a desktop probe");
        })
        .await
        .unwrap();
    assert!(supervisor.desktop_process.is_none());
    assert_eq!(supervisor.desktop_state(), None);
}

#[tokio::test(start_paused = true)]
async fn first_ready_probe_completes_without_sleeping() {
    for extended in [false, true] {
        let (_root, supervisor) = startup(true, extended);
        let started = Instant::now();
        let supervisor = Supervisor::finish_startup_with_probe(supervisor, || ready(true))
            .await
            .unwrap();
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert!(supervisor.wait_for_desktop_ready().await);
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(supervisor.desktop_state(), Some(DesktopState::Ready));
    }
}

#[tokio::test(start_paused = true)]
async fn desktop_ready_after_thirty_seconds_needs_the_extended_budget() {
    let listener_ready = Duration::from_secs(31);
    for extended in [false, true] {
        let (_root, supervisor) = startup(true, extended);
        let started = Instant::now();
        let supervisor = Supervisor::finish_startup_with_probe(supervisor, move || {
            ready(started.elapsed() >= listener_ready)
        })
        .await
        .unwrap();
        // Control plane startup returns immediately before desktop readiness.
        assert_eq!(started.elapsed(), Duration::ZERO);
        let ready = supervisor.wait_for_desktop_ready().await;
        if extended {
            assert!(ready);
            assert_eq!(supervisor.desktop_state(), Some(DesktopState::Ready));
            assert!((listener_ready..=listener_ready + DESKTOP_PROBE_INTERVAL)
                .contains(&started.elapsed()));
        } else {
            assert!(!ready);
            assert_desktop_failed(&supervisor);
            assert_eq!(started.elapsed(), Duration::from_secs(30));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_tcp_probe_cannot_exceed_the_ninety_second_budget() {
    let (_root, supervisor) = startup(true, true);
    let started = Instant::now();
    let supervisor = Supervisor::finish_startup_with_probe(supervisor, pending::<bool>)
        .await
        .unwrap();
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert!(!supervisor.wait_for_desktop_ready().await);
    assert_desktop_failed(&supervisor);
    assert_eq!(started.elapsed(), Duration::from_secs(90));
}

#[cfg(unix)]
#[tokio::test]
async fn deadline_drops_the_managed_desktop_and_its_background_child() {
    use crate::desktop_process::tests::{assert_child_output_closed, child_with_open_output};

    let (_root, mut supervisor) = startup(true, true);
    let (desktop, output) = child_with_open_output().await;
    Arc::get_mut(&mut supervisor).unwrap().desktop_process = Some(DesktopSession::Legacy(
        Box::new(LegacyDesktop::new(desktop)),
    ));
    tokio::time::pause();
    let supervisor = Supervisor::finish_startup_with_probe(supervisor, || ready(false))
        .await
        .unwrap();
    assert!(!supervisor.wait_for_desktop_ready().await);
    assert_desktop_failed(&supervisor);
    tokio::time::resume();
    assert_child_output_closed(output).await;
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_an_unanswered_probe_drops_the_managed_desktop() {
    use crate::desktop_process::tests::{assert_child_output_closed, child_with_open_output};

    let (_root, mut supervisor) = startup(true, true);
    let (desktop, output) = child_with_open_output().await;
    Arc::get_mut(&mut supervisor).unwrap().desktop_process = Some(DesktopSession::Legacy(
        Box::new(LegacyDesktop::new(desktop)),
    ));
    let (entered, probe_started) = tokio::sync::oneshot::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let supervisor = Supervisor::finish_startup_with_probe(supervisor, move || {
        if let Some(sender) = entered.lock().unwrap().take() {
            let _ = sender.send(());
        }
        pending::<bool>()
    })
    .await
    .unwrap();
    probe_started.await.unwrap();
    drop(supervisor);
    assert_child_output_closed(output).await;
}

#[cfg(unix)]
#[tokio::test]
async fn early_desktop_child_exit_fails_fast_without_waiting_for_startup_budget() {
    use crate::desktop_process::tests::exiting_child;

    let (_root, mut supervisor) = startup(true, true);
    Arc::get_mut(&mut supervisor).unwrap().desktop_process = Some(DesktopSession::Legacy(
        Box::new(LegacyDesktop::new(exiting_child(1))),
    ));
    let started = std::time::Instant::now();
    let supervisor = Supervisor::finish_startup_with_probe(supervisor, pending::<bool>)
        .await
        .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(3), supervisor.wait_for_desktop_ready())
        .await
        .expect("early desktop child exit must not wait for startup budget");
    assert!(!ready);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_desktop_failed(&supervisor);
}

#[tokio::test]
async fn serves_readyz_and_runtime_status_immediately_while_desktop_starts() {
    let (_root, supervisor) = startup(true, false);
    let gate = Arc::new(AtomicBool::new(false));
    let probe_gate = Arc::clone(&gate);
    let supervisor = Supervisor::finish_startup_with_probe(supervisor, move || {
        ready(probe_gate.load(Ordering::Acquire))
    })
    .await
    .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = crate::link::router(Arc::clone(&supervisor));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    // Control plane is immediately serving while desktop is still starting.
    let readyz: serde_json::Value = client
        .get(format!("http://{address}/readyz"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(readyz["status"], "ready");
    assert_eq!(readyz["desktop"], "starting");

    let status_frame = RequestFrame::RuntimeStatus(RuntimeStatusRequest {
        sandbox_id: supervisor.config.sandbox_id.clone(),
        generation: supervisor.config.generation,
        home_identity: supervisor.config.home_identity.clone().unwrap(),
    });
    let control = client
        .post(format!("http://{address}/runtime/control"))
        .json(&status_frame)
        .send()
        .await
        .unwrap();
    assert_eq!(control.status(), reqwest::StatusCode::OK);
    let ResponseFrame::RuntimeStatus(status) = control.json().await.unwrap() else {
        panic!("expected RuntimeStatus while desktop is still starting");
    };
    assert_eq!(status.sandbox_id, supervisor.config.sandbox_id);

    // /runtime/desktop refuses with retryable 503 + Retry-After until ready.
    let refused = tokio_tungstenite::connect_async(format!("ws://{address}/runtime/desktop"))
        .await
        .expect_err("desktop websocket must refuse while starting");
    let tokio_tungstenite::tungstenite::Error::Http(response) = refused else {
        panic!("expected HTTP 503 refusal while desktop is starting");
    };
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok()),
        Some("1")
    );
    assert!(response.headers().get(DESKTOP_STATE_HEADER).is_none());
    let body: serde_json::Value =
        serde_json::from_slice(response.body().as_deref().unwrap()).unwrap();
    assert_eq!(body["code"], "desktop_unavailable");
    assert_eq!(body["retryable"], true);

    // Once desktop probe succeeds, readiness flips to ready and generation unlocks.
    gate.store(true, Ordering::Release);
    assert!(
        tokio::time::timeout(Duration::from_secs(2), supervisor.wait_for_desktop_ready())
            .await
            .unwrap()
    );
    let readyz: serde_json::Value = client
        .get(format!("http://{address}/readyz"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(readyz["desktop"], "ready");
    let generation = supervisor
        .desktop_generation()
        .expect("machine desktop")
        .expect("ready desktop generation");
    assert_eq!(generation.token(), "desktop-startup-token");

    server.abort();
    let _ = server.await;
}

#[cfg(unix)]
#[tokio::test]
async fn failed_initial_desktop_marks_desktop_route_terminal_while_control_stays_healthy() {
    use crate::desktop_process::tests::exiting_child;

    let (_root, mut supervisor) = startup(true, false);
    Arc::get_mut(&mut supervisor).unwrap().desktop_process = Some(DesktopSession::Legacy(
        Box::new(LegacyDesktop::new(exiting_child(1))),
    ));
    let supervisor = Supervisor::finish_startup_with_probe(supervisor, pending::<bool>)
        .await
        .unwrap();
    assert!(
        !tokio::time::timeout(Duration::from_secs(3), supervisor.wait_for_desktop_ready())
            .await
            .unwrap()
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = crate::link::router(Arc::clone(&supervisor));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();

    let readyz: serde_json::Value = client
        .get(format!("http://{address}/readyz"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(readyz["desktop"], "failed");

    let status_frame = RequestFrame::RuntimeStatus(RuntimeStatusRequest {
        sandbox_id: supervisor.config.sandbox_id.clone(),
        generation: supervisor.config.generation,
        home_identity: supervisor.config.home_identity.clone().unwrap(),
    });
    let control = client
        .post(format!("http://{address}/runtime/control"))
        .json(&status_frame)
        .send()
        .await
        .unwrap();
    assert_eq!(control.status(), reqwest::StatusCode::OK);

    let refused = tokio_tungstenite::connect_async(format!("ws://{address}/runtime/desktop"))
        .await
        .expect_err("failed desktop must reject upgrades");
    let tokio_tungstenite::tungstenite::Error::Http(response) = refused else {
        panic!("expected HTTP 503 terminal refusal");
    };
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        response
            .headers()
            .get(DESKTOP_STATE_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some(DESKTOP_STATE_FAILED)
    );
    assert!(response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .is_none());
    let body: serde_json::Value =
        serde_json::from_slice(response.body().as_deref().unwrap()).unwrap();
    assert_eq!(body["code"], "desktop_unavailable");
    assert_eq!(body["retryable"], false);

    server.abort();
    let _ = server.await;
}
