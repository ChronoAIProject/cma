//! Blocking work while the OS refuses new threads.
//!
//! Each scenario lowers `RLIMIT_NPROC`, so it runs in a child process that
//! re-executes this test binary; the harness never runs under the limit. The
//! two unwarmed controls prove the limit really produces Tokio's failure
//! modes, so the prewarmed scenario is not passing by accident.
#![cfg(target_os = "linux")]

use std::{
    process::{Command, Output},
    time::Duration,
};
use tokio::runtime::Builder;

const CHILD: &str = "CMAEG_BLOCKING_POOL_CHILD";

/// Re-execute this binary to run one ignored child scenario, or skip as root.
fn run_child(name: &str) -> Option<Output> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        println!("skipping {name}: root bypasses RLIMIT_NPROC");
        return None;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env(CHILD, "1")
        .output()
        .unwrap();
    // An `--exact` filter that matches nothing also exits 0.
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("running 1 test"),
        "child scenario {name} did not run\n{}",
        describe(&output)
    );
    Some(output)
}

fn in_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

fn describe(output: &Output) -> String {
    format!(
        "{}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Lower this process's thread allowance below what the user already runs.
fn refuse_new_threads() {
    let limit = libc::rlimit {
        rlim_cur: 1,
        rlim_max: 1,
    };
    // SAFETY: `limit` is a live, initialised rlimit for the duration of the call.
    let result = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &limit) };
    assert_eq!(result, 0, "setrlimit: {}", std::io::Error::last_os_error());
    let refused = std::thread::Builder::new()
        .spawn(|| ())
        .map(|thread| thread.join())
        .expect_err("the OS still creates threads under RLIMIT_NPROC=1");
    // Tokio's queueing fallback applies only to this temporary error kind.
    assert_eq!(refused.kind(), std::io::ErrorKind::WouldBlock);
}

#[test]
fn prewarmed_pool_queues_blocking_work_when_threads_cannot_spawn() {
    let Some(output) = run_child("prewarmed_pool_child") else {
        return;
    };
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
#[ignore = "child of prewarmed_pool_queues_blocking_work_when_threads_cannot_spawn"]
fn prewarmed_pool_child() {
    if !in_child() {
        return;
    }
    let runtime = cmaeg::runtime::build().unwrap();
    refuse_new_threads();
    runtime.block_on(async {
        let tasks: Vec<_> = (0..64)
            .map(|_| tokio::task::spawn_blocking(|| std::thread::sleep(Duration::from_millis(10))))
            .collect();
        tokio::time::timeout(Duration::from_secs(5), async {
            for task in tasks {
                task.await.unwrap();
            }
        })
        .await
        .expect("64 blocking tasks did not complete within 5 s");
    });
}

#[test]
fn unwarmed_multi_thread_pool_stalls_when_threads_cannot_spawn() {
    let Some(output) = run_child("unwarmed_multi_thread_child") else {
        return;
    };
    assert!(output.status.success(), "{}", describe(&output));
}

#[test]
#[ignore = "child of unwarmed_multi_thread_pool_stalls_when_threads_cannot_spawn"]
fn unwarmed_multi_thread_child() {
    if !in_child() {
        return;
    }
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .thread_keep_alive(Duration::from_millis(100))
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::task::spawn_blocking(|| ()).await.unwrap();
        // Its thread expires, leaving the runtime's workers as the pool's only threads.
        tokio::time::sleep(Duration::from_millis(300)).await;
        refuse_new_threads();
        let outcome =
            tokio::time::timeout(Duration::from_secs(3), tokio::task::spawn_blocking(|| ())).await;
        assert!(
            outcome.is_err(),
            "the blocking task ran although no thread could be created"
        );
    });
    // The queued task never runs, so the runtime must not wait for it.
    runtime.shutdown_background();
}

#[test]
fn unwarmed_current_thread_pool_panics() {
    let Some(output) = run_child("unwarmed_current_thread_child") else {
        return;
    };
    assert!(!output.status.success(), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("OS can't spawn worker thread"),
        "{}",
        describe(&output)
    );
}

#[test]
#[ignore = "child of unwarmed_current_thread_pool_panics"]
fn unwarmed_current_thread_child() {
    if !in_child() {
        return;
    }
    let runtime = Builder::new_current_thread().enable_all().build().unwrap();
    refuse_new_threads();
    drop(runtime.spawn_blocking(|| ()));
}
