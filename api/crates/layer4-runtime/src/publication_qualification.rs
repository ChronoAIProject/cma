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
    use std::io::Read;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    struct OwnedChild(Child);

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            // Non-reaping observation leaves our child's PID reserved. Signal
            // only while Child still reports it as our live child.
            if matches!(self.0.try_wait(), Ok(None)) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
    }

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
        let mut child = OwnedChild(
            Command::new(std::env::current_exe().unwrap())
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
                .unwrap(),
        );
        let pid = child.0.id() as libc::pid_t;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut stopped = false;
        while Instant::now() < deadline {
            let mut status: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let observed = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut status,
                    libc::WSTOPPED | libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            assert_eq!(observed, 0, "cannot observe owned qualification child");
            if status.si_signo != 0 {
                stopped = status.si_code == libc::CLD_STOPPED
                    && unsafe { status.si_status() } == libc::SIGSTOP;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !stopped {
            panic!("qualification child did not reach the actual SIGSTOP boundary");
        }
        assert_eq!(unsafe { libc::kill(pid, libc::SIGCONT) }, 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.0.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                panic!("continued qualification child failed to refuse serving");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = child.0.wait().unwrap();
        let mut stderr = Vec::new();
        child
            .0
            .stderr
            .take()
            .unwrap()
            .read_to_end(&mut stderr)
            .unwrap();
        assert!(status.success(), "{status:?}");
        assert_eq!(stderr, BOUNDARY);
    }
}
