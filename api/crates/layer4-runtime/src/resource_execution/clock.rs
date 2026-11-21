use layer3_adapter::resource_execution_protocol::{ProtocolError, ProtocolResult};

pub(super) trait Clock: Send + Sync {
    fn tick_ms(&self) -> ProtocolResult<u64>;
}

pub(super) struct BootClock;
impl Clock for BootClock {
    fn tick_ms(&self) -> ProtocolResult<u64> {
        #[cfg(target_os = "linux")]
        {
            let mut time = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            // CLOCK_BOOTTIME includes suspend and is scoped to this OS boot.
            if unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut time) } != 0 {
                return Err(ProtocolError::Unavailable);
            }
            u64::try_from(time.tv_sec)
                .ok()
                .and_then(|seconds| seconds.checked_mul(1000))
                .and_then(|ms| {
                    u64::try_from(time.tv_nsec)
                        .ok()
                        .and_then(|ns| ms.checked_add(ns / 1_000_000))
                })
                .ok_or(ProtocolError::Unavailable)
        }
        #[cfg(not(target_os = "linux"))]
        Err(ProtocolError::Unsupported)
    }
}
