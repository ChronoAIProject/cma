//! The watchdog loop over a fake gauge and signaller, on every platform.
use super::*;
use std::{fs::File, os::fd::AsRawFd, sync::atomic::AtomicUsize, time::Instant};

/// Reports `usage` and `inactive`, counting samples.
#[derive(Clone, Default)]
struct FakeGauge {
    usage: Arc<AtomicU64>,
    inactive: Arc<AtomicU64>,
    samples: Arc<AtomicUsize>,
}

const LIMIT: u64 = 1_000_000;

impl MemoryGauge for FakeGauge {
    fn limit(&self) -> u64 {
        LIMIT
    }

    fn sample(&mut self, _job_pid: u32) -> io::Result<u64> {
        self.samples.fetch_add(1, Ordering::SeqCst);
        Ok(self.usage.load(Ordering::SeqCst))
    }

    fn inactive_file(&mut self) -> io::Result<u64> {
        Ok(self.inactive.load(Ordering::SeqCst))
    }
}

/// Records the raw descriptor of every kill.
#[derive(Clone, Default)]
struct FakeSignaller {
    kills: Arc<Mutex<Vec<i32>>>,
}

impl Signaller for FakeSignaller {
    fn kill(&mut self, pidfd: BorrowedFd<'_>) -> io::Result<()> {
        self.kills.lock().unwrap().push(pidfd.as_raw_fd());
        Ok(())
    }
}

fn start(gauge: &FakeGauge, signaller: &FakeSignaller) -> WatchdogHandle {
    Watchdog::start(Box::new(gauge.clone()), Box::new(signaller.clone())).unwrap()
}

fn descriptor() -> OwnedFd {
    OwnedFd::from(File::open(std::env::current_exe().unwrap()).unwrap())
}

/// Wait until `condition` holds, for at most a second.
fn eventually(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        thread::sleep(PERIOD);
    }
    condition()
}

#[test]
fn fires_at_threshold_after_subtracting_inactive_file() {
    let (gauge, signaller) = (FakeGauge::default(), FakeSignaller::default());
    let watchdog = start(&gauge, &signaller);
    assert_eq!(watchdog.threshold(), 900_000);
    gauge.usage.store(950_000, Ordering::SeqCst);
    gauge.inactive.store(50_000, Ordering::SeqCst);
    let registration = watchdog.register(7, descriptor());
    assert!(eventually(|| !signaller.kills.lock().unwrap().is_empty()));
    assert!(registration.finish());
    assert_eq!(signaller.kills.lock().unwrap().len(), 1);
}

#[test]
fn does_not_fire_when_inactive_file_brings_it_below() {
    let (gauge, signaller) = (FakeGauge::default(), FakeSignaller::default());
    let watchdog = start(&gauge, &signaller);
    gauge.usage.store(950_000, Ordering::SeqCst);
    gauge.inactive.store(50_001, Ordering::SeqCst);
    let registration = watchdog.register(7, descriptor());
    assert!(eventually(|| gauge.samples.load(Ordering::SeqCst) >= 5));
    assert!(!registration.finish());
    assert!(signaller.kills.lock().unwrap().is_empty());
}

#[test]
fn does_not_fire_below_threshold() {
    let (gauge, signaller) = (FakeGauge::default(), FakeSignaller::default());
    let watchdog = start(&gauge, &signaller);
    gauge.usage.store(899_999, Ordering::SeqCst);
    let registration = watchdog.register(7, descriptor());
    assert!(eventually(|| gauge.samples.load(Ordering::SeqCst) >= 5));
    assert!(!registration.finish());
    assert!(signaller.kills.lock().unwrap().is_empty());
}

#[test]
fn signals_only_through_the_registered_pidfd() {
    let (gauge, signaller) = (FakeGauge::default(), FakeSignaller::default());
    let watchdog = start(&gauge, &signaller);
    gauge.usage.store(LIMIT, Ordering::SeqCst);
    let pidfd = descriptor();
    let raw = pidfd.as_raw_fd();
    let registration = watchdog.register(7, pidfd);
    assert!(eventually(|| !signaller.kills.lock().unwrap().is_empty()));
    assert!(registration.finish());
    assert_eq!(*signaller.kills.lock().unwrap(), [raw]);
}

#[test]
fn no_signal_after_finish() {
    let (gauge, signaller) = (FakeGauge::default(), FakeSignaller::default());
    let watchdog = start(&gauge, &signaller);
    let registration = watchdog.register(7, descriptor());
    assert!(eventually(|| gauge.samples.load(Ordering::SeqCst) >= 2));
    assert!(!registration.finish());
    gauge.usage.store(LIMIT, Ordering::SeqCst);
    thread::sleep(PERIOD * 10);
    assert!(signaller.kills.lock().unwrap().is_empty());
    // A dropped registration deregisters too.
    let dropped = watchdog.register(8, descriptor());
    drop(dropped);
    gauge.usage.store(0, Ordering::SeqCst);
    let samples = gauge.samples.load(Ordering::SeqCst);
    gauge.usage.store(LIMIT, Ordering::SeqCst);
    thread::sleep(PERIOD * 10);
    assert!(gauge.samples.load(Ordering::SeqCst) <= samples + 1);
    assert!(signaller.kills.lock().unwrap().is_empty());
}

#[test]
fn parks_without_jobs() {
    let (gauge, signaller) = (FakeGauge::default(), FakeSignaller::default());
    let watchdog = start(&gauge, &signaller);
    thread::sleep(PERIOD * 10);
    assert_eq!(gauge.samples.load(Ordering::SeqCst), 0);
    let registration = watchdog.register(7, descriptor());
    assert!(eventually(|| gauge.samples.load(Ordering::SeqCst) > 0));
    registration.finish();
    // A loop iteration that read the registration just before `finish` may
    // still take its one sample; after that the thread parks.
    let at_finish = gauge.samples.load(Ordering::SeqCst);
    thread::sleep(PERIOD * 10);
    let idle = gauge.samples.load(Ordering::SeqCst);
    assert!(idle <= at_finish + 1, "{idle} samples after {at_finish}");
    thread::sleep(PERIOD * 10);
    assert_eq!(gauge.samples.load(Ordering::SeqCst), idle);
}

#[test]
fn a_disabled_gauge_never_fires() {
    let signaller = FakeSignaller::default();
    let watchdog = Watchdog::start(Box::new(DisabledGauge), Box::new(signaller.clone())).unwrap();
    assert_eq!(watchdog.limit(), u64::MAX);
    let registration = watchdog.register(7, descriptor());
    thread::sleep(PERIOD * 5);
    assert!(!registration.finish());
    assert!(signaller.kills.lock().unwrap().is_empty());
    assert_eq!(watchdog.sample_errors(), 0);
}

#[test]
fn unreadable_samples_are_counted_not_fatal() {
    struct Broken;
    impl MemoryGauge for Broken {
        fn limit(&self) -> u64 {
            LIMIT
        }
        fn sample(&mut self, _job_pid: u32) -> io::Result<u64> {
            Err(io::ErrorKind::InvalidData.into())
        }
        fn inactive_file(&mut self) -> io::Result<u64> {
            Ok(0)
        }
    }
    let signaller = FakeSignaller::default();
    let watchdog = Watchdog::start(Box::new(Broken), Box::new(signaller.clone())).unwrap();
    let registration = watchdog.register(7, descriptor());
    assert!(eventually(|| watchdog.sample_errors() > 0));
    assert!(!registration.finish());
    assert!(signaller.kills.lock().unwrap().is_empty());
}
