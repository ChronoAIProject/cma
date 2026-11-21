//! The process panic hook logs where a panic happened, never its payload.
//!
//! A panic hook is process-wide, so each scenario runs in a child process that
//! re-executes this test binary. The control proves the payload would reach
//! the output without the hook, so the main scenario cannot pass by accident.
use std::process::Command;

const CHILD: &str = "CMAEG_PANIC_HOOK_CHILD";
const PAYLOAD: &str = "PANIC-PAYLOAD-SHOULD-NOT-LOG";

/// Re-execute this binary to run one ignored child scenario; returns its stdout and stderr.
fn run_child(name: &str) -> String {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env(CHILD, "1")
        .env_remove("RUST_LOG")
        .output()
        .unwrap();
    let text = format!(
        "{}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "{text}");
    // An `--exact` filter that matches nothing also exits 0.
    assert!(text.contains("running 1 test"), "{text}");
    text
}

fn in_child() -> bool {
    std::env::var_os(CHILD).is_some()
}

fn panic_on_a_thread() {
    let joined = std::thread::spawn(|| panic!("{PAYLOAD}")).join();
    assert!(joined.is_err());
}

#[test]
fn the_panic_hook_logs_the_location_without_the_payload() {
    let output = run_child("installed_hook_child");
    let logged = output
        .lines()
        .find(|line| line.contains("ERROR cmaeg: panic location="))
        .unwrap_or_else(|| panic!("no panic log: {output}"));
    assert!(logged.contains("panic_hook.rs:"), "{output}");
    assert!(!output.contains(PAYLOAD), "{output}");
}

#[test]
#[ignore = "child of the_panic_hook_logs_the_location_without_the_payload"]
fn installed_hook_child() {
    if !in_child() {
        return;
    }
    cmaeg::init_logging();
    cmaeg::install_panic_hook();
    panic_on_a_thread();
}

#[test]
fn the_default_hook_prints_the_payload() {
    let output = run_child("default_hook_child");
    assert!(output.contains(PAYLOAD), "{output}");
}

#[test]
#[ignore = "child of the_default_hook_prints_the_payload"]
fn default_hook_child() {
    if !in_child() {
        return;
    }
    cmaeg::init_logging();
    panic_on_a_thread();
}
