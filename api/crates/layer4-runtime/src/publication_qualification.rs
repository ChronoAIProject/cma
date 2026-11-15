//! Explicit local qualification mode for interrupting the actual release ELF.
//! Ordinary startup never arms this stop; no environment or wire request can.
use std::ffi::OsString;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::failure::{Result, RuntimeError};

const COMMAND: &str = "--qualify-first-publication-interruption";
const BOUNDARY: &[u8] = b"{\"qualification\":\"first_publication_interruption\",\"phase\":\"stage_synced_before_intent\",\"action\":\"sigstop\"}\n";
static ARMED: AtomicBool = AtomicBool::new(false);

pub(crate) fn requested(arguments: &[OsString]) -> bool {
    arguments.len() == 1 && arguments[0] == COMMAND
}

pub(crate) fn arm() {
    ARMED.store(true, Ordering::SeqCst);
}

/// Called only in new publication after the staged DB has closed and synced,
/// before any intent is created. The harness must observe the stopped process,
/// kill it, then restart the normal command on the same retained volume.
pub(crate) fn stop_before_intent() -> Result<()> {
    if !ARMED.swap(false, Ordering::SeqCst) {
        return Ok(());
    }
    {
        let mut stderr = std::io::stderr().lock();
        stderr
            .write_all(BOUNDARY)
            .and_then(|()| stderr.flush())
            .map_err(|_| refusal("cannot record publication qualification boundary"))?;
    }
    // SIGSTOP cannot be handled or ignored. Log emission alone is not proof
    // that it took effect: the external owner must observe the stopped PID.
    if unsafe { libc::raise(libc::SIGSTOP) } != 0 {
        return Err(refusal("cannot stop at publication qualification boundary"));
    }
    // An accidental SIGCONT must not let a qualification lifetime serve.
    Err(refusal(
        "publication qualification requires a normal restart",
    ))
}

fn refusal(detail: &'static str) -> RuntimeError {
    RuntimeError::rejected("publication_qualification_stopped", detail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn ordinary_and_mixed_commands_cannot_arm_qualification() {
        for arguments in [
            vec![],
            vec![OsString::from("--wait-for-assignment")],
            vec![OsString::from("--assign-runtime")],
            vec![OsString::from(COMMAND), OsString::from("extra")],
        ] {
            assert!(!requested(&arguments));
        }
        assert!(requested(&[OsString::from(COMMAND)]));
        // This test process was never armed, so normal publication proceeds.
        stop_before_intent().unwrap();
    }

    #[test]
    #[ignore = "isolated child of qualification_stop_is_real_and_continue_refuses"]
    fn qualification_child() {
        arm();
        let error = stop_before_intent().unwrap_err();
        assert_eq!(error.code, "publication_qualification_stopped");
        assert_eq!(
            error.detail,
            "publication qualification requires a normal restart"
        );
        assert!(!error.retryable);
        // The stop is one-shot within this explicit qualification process.
        stop_before_intent().unwrap();
    }

    #[test]
    fn qualification_stop_is_real_and_continue_refuses() {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "publication_qualification::tests::qualification_child",
                "--ignored",
                "--nocapture",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id() as libc::pid_t;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stopped = false;
        while Instant::now() < deadline {
            let mut status = 0;
            let observed =
                unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED | libc::WNOHANG) };
            if observed == pid {
                stopped = libc::WIFSTOPPED(status) && libc::WSTOPSIG(status) == libc::SIGSTOP;
                break;
            }
            if observed < 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !stopped {
            let _ = child.kill();
            let _ = child.wait();
            panic!("qualification child did not reach the actual SIGSTOP boundary");
        }
        assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("continued qualification child failed to refuse serving");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stderr, BOUNDARY);
    }
}
