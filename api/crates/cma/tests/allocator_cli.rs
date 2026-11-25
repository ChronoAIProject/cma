//! The shipped API binary qualifies its allocator without starting the service.
use std::{ffi::OsStr, process::Output};

fn run(arguments: &[&OsStr]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_cma"))
        .args(arguments)
        .env_clear()
        .output()
        .expect("run the compiled API executable")
}

fn assert_refusal(output: Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(output.stderr, b"usage: cma --allocator-self-test\n");
}

#[test]
fn allocator_qualification_rejects_extra_arguments_without_starting() {
    for extra in ["extra", "--allocator-self-test", "--print-openapi"] {
        assert_refusal(run(&[
            OsStr::new("--allocator-self-test"),
            OsStr::new(extra),
        ]));
    }
}

#[cfg(unix)]
#[test]
fn allocator_qualification_rejects_non_utf8_extra_arguments() {
    use std::os::unix::ffi::OsStrExt;

    assert_refusal(run(&[
        OsStr::new("--allocator-self-test"),
        OsStr::from_bytes(b"\xff"),
    ]));
}

#[test]
fn ordinary_usage_advertises_allocator_qualification() {
    let output = run(&[OsStr::new("--unknown-command")]);
    assert_eq!(output.status.code(), Some(2));
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("usage: cma "), "{error}");
    assert!(error.contains("--allocator-self-test"), "{error}");
    assert!(!error.contains("outcome=ok"));
}

#[cfg(not(target_os = "linux"))]
#[test]
fn missing_procfs_fails_before_configuration_or_serving() {
    let output = run(&[OsStr::new("--allocator-self-test")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.starts_with("allocator-self-test outcome=failed error="));
    assert_eq!(error.lines().count(), 1, "{error}");
}

#[cfg(target_os = "linux")]
#[test]
fn actual_api_allocator_completes_without_configuration() {
    let output = run(&[OsStr::new("--allocator-self-test")]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "{stderr}");
    assert!(stdout.contains(
        "workers=4 blocks_per_worker=128 rounds=4096 warmup=256 retained_limit_kib=24576"
    ));
    for boundary in [
        "256", "512", "1024", "1536", "2048", "2560", "3072", "3584", "4096", "quiesced",
    ] {
        assert!(
            stdout.contains(&format!("boundary={boundary} live_blocks=0 rss_kib=")),
            "{stdout}"
        );
    }
    assert!(
        stdout.ends_with("allocator-self-test outcome=ok\n"),
        "{stdout}"
    );
}
