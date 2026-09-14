use super::*;

#[test]
fn an_empty_writer_set_can_always_be_fenced() {
    let mut fence = WriterFence::quiesce([], SupervisorBoundary::default()).unwrap();
    assert_eq!(fence.writer_count(), 0);
    fence.resume().unwrap();
    fence.resume().unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn a_process_session_is_stopped_and_resumed() {
    use std::os::unix::process::CommandExt as _;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "sleep 30 & wait"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let root = i32::try_from(child.id()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let table = crate::writer_process::process_table().unwrap();
        if table.values().any(|record| record.session == root) {
            break;
        }
        assert!(Instant::now() < deadline);
    }
    let mut fence = WriterFence::quiesce([root], SupervisorBoundary::default()).unwrap();
    assert!(fence.writer_count() >= 1);
    fence.resume().unwrap();
    unsafe {
        libc::kill(-root, libc::SIGKILL);
    }
    let _ = child.wait();
}
