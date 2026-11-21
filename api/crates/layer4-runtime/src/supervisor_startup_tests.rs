use std::collections::BTreeMap;
use std::future::{pending, ready};
use std::path::PathBuf;

use tokio::time::Instant;

use super::*;

struct TestRoot(PathBuf);

impl Drop for TestRoot {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).expect("remove desktop startup fixture");
    }
}

fn startup(machine: bool, extended: bool) -> (TestRoot, Arc<Supervisor>) {
    let path = std::env::temp_dir().join(format!("cma-desktop-startup-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&path).unwrap();
    let root = TestRoot(path);
    let mut config = RuntimeConfig::for_test(root.0.join("home"));
    config.extended_desktop_startup = extended;
    let mut supervisor = Supervisor::new(config, SupervisorBoundary::default()).unwrap();
    // The tests supply the readiness probe and, where needed, a managed child.
    Arc::get_mut(&mut supervisor)
        .unwrap()
        .config
        .machine_included = machine;
    (root, supervisor)
}

fn assert_desktop_timeout(result: Result<Arc<Supervisor>>) {
    let Err(error) = result else {
        panic!("desktop startup exceeded its budget");
    };
    assert_eq!(error.code, "desktop_start_failed");
    assert_eq!(error.detail, "desktop did not become ready");
    assert!(error.retryable);
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
}

#[tokio::test(start_paused = true)]
async fn first_ready_probe_completes_without_sleeping() {
    for extended in [false, true] {
        let (_root, supervisor) = startup(true, extended);
        let started = Instant::now();
        Supervisor::finish_startup_with_probe(supervisor, || ready(true))
            .await
            .unwrap();
        assert_eq!(started.elapsed(), Duration::ZERO);
    }
}

#[tokio::test(start_paused = true)]
async fn desktop_ready_after_thirty_seconds_needs_the_extended_budget() {
    let listener_ready = Duration::from_secs(31);
    for extended in [false, true] {
        let (_root, supervisor) = startup(true, extended);
        let started = Instant::now();
        let result = Supervisor::finish_startup_with_probe(supervisor, || {
            ready(started.elapsed() >= listener_ready)
        })
        .await;
        if extended {
            assert!(result.is_ok());
            assert!((listener_ready..=listener_ready + DESKTOP_PROBE_INTERVAL)
                .contains(&started.elapsed()));
        } else {
            assert_desktop_timeout(result);
            assert_eq!(started.elapsed(), Duration::from_secs(30));
        }
    }
}

#[tokio::test(start_paused = true)]
async fn an_unanswered_tcp_probe_cannot_exceed_the_ninety_second_budget() {
    let (_root, supervisor) = startup(true, true);
    let started = Instant::now();
    assert_desktop_timeout(
        Supervisor::finish_startup_with_probe(supervisor, pending::<bool>).await,
    );
    assert_eq!(started.elapsed(), Duration::from_secs(90));
}

#[cfg(unix)]
#[tokio::test]
async fn deadline_drops_the_managed_desktop_and_its_background_child() {
    use crate::desktop_process::tests::{assert_child_output_closed, child_with_open_output};

    let (_root, mut supervisor) = startup(true, true);
    let (desktop, output) = child_with_open_output().await;
    Arc::get_mut(&mut supervisor).unwrap().desktop_process = Some(
        crate::desktop_process::DesktopSession::Legacy(Box::new(desktop)),
    );
    tokio::time::pause();
    assert_desktop_timeout(
        Supervisor::finish_startup_with_probe(supervisor, || ready(false)).await,
    );
    tokio::time::resume();
    assert_child_output_closed(output).await;
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_an_unanswered_probe_drops_the_managed_desktop() {
    use crate::desktop_process::tests::{assert_child_output_closed, child_with_open_output};

    let (_root, mut supervisor) = startup(true, true);
    let (desktop, output) = child_with_open_output().await;
    Arc::get_mut(&mut supervisor).unwrap().desktop_process = Some(
        crate::desktop_process::DesktopSession::Legacy(Box::new(desktop)),
    );
    let (entered, probe_started) = tokio::sync::oneshot::channel();
    let mut entered = Some(entered);
    let startup = tokio::spawn(Supervisor::finish_startup_with_probe(
        supervisor,
        move || {
            entered.take().unwrap().send(()).unwrap();
            pending::<bool>()
        },
    ));
    probe_started.await.unwrap();
    startup.abort();
    assert!(matches!(startup.await, Err(error) if error.is_cancelled()));
    assert_child_output_closed(output).await;
}
