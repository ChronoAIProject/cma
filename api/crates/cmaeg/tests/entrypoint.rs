//! Process argument dispatch: unknown arguments are a usage error, never a panic.
use std::{ffi::OsStr, process::Output};

const USAGE: &str = "usage: cmaeg [--print-openapi | --audit-custom-attachments [--clear]]\n";

fn run(arguments: &[&OsStr]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_cmaeg"))
        .args(arguments)
        .env_clear()
        .output()
        .unwrap()
}

fn assert_usage(output: &Output) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert_eq!(String::from_utf8_lossy(&output.stderr), USAGE);
}

#[test]
fn unknown_arguments_print_the_usage_line_and_exit_two() {
    assert_usage(&run(&[OsStr::new("--serve")]));
    assert_usage(&run(&[OsStr::new("--print-openapi"), OsStr::new("extra")]));
}

#[cfg(unix)]
#[test]
fn a_non_utf8_argument_is_a_usage_error_instead_of_a_panic() {
    use std::os::unix::ffi::OsStrExt;

    assert_usage(&run(&[OsStr::from_bytes(b"--print-openapi\xff")]));
    assert_usage(&run(&[
        OsStr::new("--print-openapi"),
        OsStr::from_bytes(b"\xff"),
    ]));
}
