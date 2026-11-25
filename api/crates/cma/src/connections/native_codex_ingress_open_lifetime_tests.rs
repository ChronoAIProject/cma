use super::*;

// Descriptor-number assertions run alone so another test cannot reuse the fd.
fn run_isolated(name: &str) -> bool {
    const CHILD: &str = "CMA_SYNTHETIC_OPEN_CHILD";
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        return false;
    }
    let test = format!("{}::{name}", module_path!().split_once("::").unwrap().1);
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test, "--test-threads=1"])
        .env(CHILD, name)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            let output = child.wait_with_output().unwrap();
            std::io::Write::write_all(&mut std::io::stdout(), &output.stdout).unwrap();
            assert!(output.status.success());
            let stdout = std::str::from_utf8(&output.stdout).unwrap();
            let expected = format!("test {test} ... ok");
            assert!(stdout.lines().any(|line| line == expected));
            assert!(stdout.lines().any(|line| {
                line.starts_with("test result: ok. 1 passed; 0 failed; 0 ignored;")
            }));
            return true;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("synthetic descriptor test timed out");
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn empty_descriptor_child_selection_is_refused() {
    assert!(std::panic::catch_unwind(|| run_isolated("missing_descriptor_child_case")).is_err());
}

#[test]
fn read_error_retains_the_handle_until_owner_drop() {
    if run_isolated("read_error_retains_the_handle_until_owner_drop") {
        return;
    }
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), b"{}").unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    let fd = opened.file.as_raw_fd();
    let CacheReadCompletion {
        file: opened,
        outcome,
    } = opened.read();
    assert!(matches!(
        outcome,
        Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount))
    ));
    // SAFETY: flag queries do not take ownership; the first is a live owner.
    assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
    drop(opened);
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );
}

#[test]
fn read_is_single_use_and_does_not_create_another_cursor() {
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    let CacheReadCompletion {
        file: opened,
        outcome,
    } = opened.read();
    drop(outcome.unwrap());
    assert!(matches!(
        opened.read().outcome,
        Err(CacheDecodeError::Refused(
            AdmissionRefusal::InvalidPlan { .. }
        ))
    ));
}

#[test]
fn prepared_abandonment_has_no_file_effect_and_returns_storage() {
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    let baseline = root.available_bytes();
    drop(prepare(&home, &input, &policy).unwrap());
    assert_eq!(root.available_bytes(), baseline);
    assert!(!fixture.path().exists());
}

#[test]
fn home_owner_is_retained_after_the_callers_handles_drop() {
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
    drop((home, input, policy));
    let CacheReadCompletion {
        file: opened,
        outcome,
    } = opened.read();
    let cache = outcome.unwrap();
    assert!(!cache.expose().is_empty());
    drop(opened);
    let cache_bytes = cache.requested_bytes();
    let before = root.available_bytes();
    drop(cache);
    assert_eq!(root.available_bytes(), before + cache_bytes);
}

#[test]
fn metadata_error_closes_the_newly_owned_descriptor() {
    if run_isolated("metadata_error_closes_the_newly_owned_descriptor") {
        return;
    }
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let baseline = root.available_bytes();
    let prepared = prepare(&home, &input, &policy).unwrap();
    let file = File::open(fixture.path()).unwrap();
    let fd = file.as_raw_fd();
    assert!(matches!(
        prepared.finish_open(file, Err(std::io::ErrorKind::PermissionDenied.into())),
        Err(CacheDecodeError::Account(SubscriptionError::InvalidAccount))
    ));
    // SAFETY: querying an integer fd takes no ownership or memory pointer.
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn local_unwind_closes_partial_resources_without_a_cleanup_ack_claim() {
    if run_isolated("local_unwind_closes_partial_resources_without_a_cleanup_ack_claim") {
        return;
    }
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let baseline = root.available_bytes();
    let fd = std::cell::Cell::new(-1);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let opened = prepare(&home, &input, &policy).unwrap().open().unwrap();
        fd.set(opened.file.as_raw_fd());
        panic!("synthetic local unwind after open");
    }));
    assert!(result.is_err());
    drop(result);
    // SAFETY: F_GETFD only observes this integer; it does not adopt the fd.
    assert_eq!(unsafe { libc::fcntl(fd.get(), libc::F_GETFD) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );
    assert_eq!(root.available_bytes(), baseline);
}

#[test]
fn descriptor_zero_is_a_valid_owned_result() {
    if run_isolated("descriptor_zero_is_a_valid_owned_result") {
        return;
    }
    let fixture = Fixture::new();
    let (_, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let prepared = prepare(&home, &input, &policy).unwrap();
    // SAFETY: only this isolated synthetic test subprocess closes its stdin.
    // No borrowed File in this test owns descriptor zero at this point.
    unsafe {
        libc::close(0);
    }
    let opened = prepared.open().unwrap();
    assert_eq!(opened.file.as_raw_fd(), 0);
    let completion = opened.read();
    assert!(completion.outcome.is_ok());
    drop(completion);
}

#[test]
fn file_read_error_retains_the_owned_descriptor_and_releases_buffer() {
    if run_isolated("file_read_error_retains_the_owned_descriptor_and_releases_buffer") {
        return;
    }
    let fixture = Fixture::new();
    let (root, policy) = policy();
    let home = fixture.home(&policy);
    let input = input(&policy);
    fs::write(fixture.path(), valid()).unwrap();
    let baseline = root.available_bytes();
    let prepared = prepare(&home, &input, &policy).unwrap();
    // A real write-only descriptor exercises an OS read error without closing
    // or replacing a descriptor behind a live File owner.
    let file = fs::OpenOptions::new()
        .write(true)
        .open(fixture.path())
        .unwrap();
    let fd = file.as_raw_fd();
    let metadata = file.metadata();
    let opened = prepared.finish_open(file, metadata).unwrap();
    let CacheReadCompletion {
        file: opened,
        outcome,
    } = opened.read();
    assert!(matches!(
        outcome,
        Err(CacheDecodeError::Account(SubscriptionError::Unavailable))
    ));
    assert_eq!(
        root.available_bytes(),
        baseline - opened.storage.requested_bytes()
    );
    // SAFETY: F_GETFD observes a descriptor number without taking ownership.
    assert!(unsafe { libc::fcntl(fd, libc::F_GETFD) } >= 0);
    drop(opened);
    assert_eq!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EBADF)
    );
    assert_eq!(root.available_bytes(), baseline);
}
