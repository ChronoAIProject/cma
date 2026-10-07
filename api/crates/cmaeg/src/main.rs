use cmaeg::media::transform::{
    argv::{MEDIA_JOB_FLAG as MEDIA_JOB, SANDBOX_CHECK_FLAG as SANDBOX_CHECK},
    self_test::SELF_TEST_FLAG as SELF_TEST,
};
use std::{ffi::OsString, io::Write, path::Path};

const USAGE: &str = "usage: cmaeg [--print-openapi | --audit-custom-attachments [--clear] | --media-job <op> <bin-dir> <job-dir> [arg] | --media-sandbox-check | --media-self-test <fixtures-dir>]";

fn main() {
    // `args_os` never panics: `args` aborts with exit 101 on a non-UTF-8
    // argument before any usage handling could run.
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    // The media job modes come first and are matched on the raw arguments:
    // a malformed job must exit 125 (sandbox unavailable), never 2 or 101.
    // Neither reads the environment, logs or builds a runtime, and a job must
    // not inherit the gateway's non-dumpable flag.
    match arguments.first().map(OsString::as_os_str) {
        Some(flag) if flag == MEDIA_JOB => {
            cmaeg::media::transform::job_main(arguments[1..].to_vec())
        }
        Some(flag) if flag == SANDBOX_CHECK && arguments.len() == 1 => {
            cmaeg::media::transform::sandbox_check_main()
        }
        _ => {}
    }
    // Then, before any other mode, log line or runtime: every other mode
    // holds or can load the gateway's secrets.
    if let Err(error) = cmaeg::hardening::disable_dumping() {
        eprintln!("cmaeg: cannot make the process non-dumpable: {error}");
        std::process::exit(1);
    }
    // Modes that need no runtime are matched on the raw arguments first, so
    // they run before one exists. The self-test builds its own, and takes
    // its fixtures path as given, UTF-8 or not.
    if arguments == ["--print-openapi"] {
        print!("{}", cmaeg::openapi::snapshot());
        return;
    }
    if let [flag, fixtures] = arguments.as_slice() {
        if flag == SELF_TEST {
            media_self_test(Path::new(fixtures));
        }
    }
    let Some(arguments) = arguments
        .iter()
        .map(|argument| argument.to_str())
        .collect::<Option<Vec<_>>>()
    else {
        usage();
    };
    match arguments.as_slice() {
        [] => serve(),
        ["--audit-custom-attachments"] => audit(false),
        ["--audit-custom-attachments", "--clear"] => audit(true),
        _ => usage(),
    }
}

/// The build-time media self-test. Its report is the only stdout; logs go
/// to stderr.
fn media_self_test(fixtures: &Path) -> ! {
    cmaeg::init_logging_to_stderr();
    cmaeg::install_panic_hook();
    cmaeg::media::transform::self_test::main(fixtures)
}

fn usage() -> ! {
    eprintln!("{USAGE}");
    std::process::exit(2);
}

fn serve() {
    cmaeg::init_logging();
    cmaeg::install_panic_hook();
    // Both failures exit through `process::exit`: threads a failed prewarm
    // left parked on its barrier end with the process, before any listener binds.
    let runtime = match cmaeg::runtime::build() {
        Ok(runtime) => runtime,
        Err(cmaeg::runtime::RuntimeError::Prewarm) => {
            tracing::error!("CMAEG blocking pool prewarm failed");
            std::process::exit(1);
        }
        Err(error) => {
            tracing::error!(%error, "CMAEG runtime unavailable");
            std::process::exit(1);
        }
    };
    if let Err(error) = runtime.block_on(cmaeg::run()) {
        tracing::error!(%error, "CMAEG stopped");
        std::process::exit(1);
    }
}

/// The rollback audit. Stdout carries only the JSON report, so logs go to
/// stderr, and an error is printed as its classified name alone. It exits 0
/// only when no setting remains and every record could be read.
fn audit(clear: bool) {
    cmaeg::init_logging_to_stderr();
    cmaeg::install_panic_hook();
    let runtime = match cmaeg::runtime::build() {
        Ok(runtime) => runtime,
        Err(cmaeg::runtime::RuntimeError::Prewarm) => fail("blocking pool prewarm failed"),
        Err(_) => fail("runtime unavailable"),
    };
    let report = match runtime.block_on(cmaeg::audit_custom_attachments(clear)) {
        Ok(report) => report,
        Err(error) => fail(&error.to_string()),
    };
    let mut stdout = std::io::stdout().lock();
    let written = report
        .lines()
        .iter()
        .try_for_each(|line| writeln!(stdout, "{line}"))
        .and_then(|()| stdout.flush());
    if written.is_err() {
        fail("stdout unavailable");
    }
    std::process::exit(if report.complete() { 0 } else { 1 });
}

fn fail(reason: &str) -> ! {
    eprintln!("{reason}");
    std::process::exit(1);
}
