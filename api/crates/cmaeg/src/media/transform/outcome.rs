//! What became of one media job, classified from facts the gateway observed
//! itself: who killed it, how it exited, the CPU it used and its output.
//!
//! The job's stderr is never kept: only a class matched on fixed substrings
//! of its first 4 KiB survives, because decoder messages can quote input.
use std::{fmt, time::Duration};

/// SIGKILL, the signal the kernel's CPU limit and every gateway kill send.
pub const SIGKILL: i32 = 9;
/// The wrapper's exit status when it could not sandbox itself.
pub const SANDBOX_EXIT: i32 = 125;
/// `RLIMIT_CPU` is 20 s; half a second of accounting slack still means the
/// limit ended the job, by SIGKILL or by FFmpeg's own SIGXCPU-style exit.
pub const CPU_LIMIT_SPENT: Duration = Duration::from_millis(19_500);
/// The stderr prefix inspected for a class.
pub const STDERR_HEAD: usize = 4096;

/// The outcome of one job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformOutcome {
    Ok,
    /// The gateway killed it at its deadline.
    Timeout,
    /// It spent its CPU limit.
    CpuLimit,
    /// The memory watchdog killed it.
    ResourceLimit,
    /// Some other SIGKILL.
    Killed,
    ExitNonzero,
    OutputInvalid,
    /// It could not be started, sandboxed or observed.
    SandboxUnavailable,
}

impl TransformOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Timeout => "timeout",
            Self::CpuLimit => "cpu_limit",
            Self::ResourceLimit => "resource_limit",
            Self::Killed => "killed",
            Self::ExitNonzero => "exit_nonzero",
            Self::OutputInvalid => "output_invalid",
            Self::SandboxUnavailable => "sandbox_unavailable",
        }
    }
}

impl fmt::Display for TransformOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// How a reaped job ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

/// What the stderr head says, as far as the reason a caller gets depends on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StderrClass {
    #[default]
    None,
    InvalidData,
    UnsupportedCodec,
    Other,
}

const INVALID_DATA: [&str; 1] = ["Invalid data found when processing input"];
const UNSUPPORTED_CODEC: [&str; 3] = [
    "Decoder not found",
    "Unknown decoder",
    "could not find codec parameters",
];

impl StderrClass {
    /// The class of `stderr`, judged on its first [`STDERR_HEAD`] bytes.
    pub fn of(stderr: &[u8]) -> Self {
        let head = &stderr[..stderr.len().min(STDERR_HEAD)];
        if head.is_empty() {
            return Self::None;
        }
        let contains = |needles: &[&str]| {
            needles.iter().any(|needle| {
                head.windows(needle.len())
                    .any(|window| window == needle.as_bytes())
            })
        };
        if contains(&INVALID_DATA) {
            Self::InvalidData
        } else if contains(&UNSUPPORTED_CODEC) {
            Self::UnsupportedCodec
        } else {
            Self::Other
        }
    }
}

/// The facts [`classify`] decides from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClassifyInput {
    /// The gateway killed the job at its deadline.
    pub deadline_killed: bool,
    /// The memory watchdog killed the job.
    pub watchdog_killed: bool,
    /// The gateway killed the job because its stdout passed the cap.
    pub overflow_killed: bool,
    pub exit: Exit,
    /// The job's CPU time, from the `RUSAGE_CHILDREN` difference.
    pub cpu: Duration,
    /// Whether the output passed the caller's validation.
    pub output_valid: bool,
}

/// Classify one reaped job, in a fixed order: the gateway's deadline kill,
/// the watchdog's kill, the gateway's output kill, the sandbox exit status,
/// the CPU limit, any other SIGKILL, any other failure, then the output.
pub fn classify(input: &ClassifyInput) -> TransformOutcome {
    if input.deadline_killed {
        TransformOutcome::Timeout
    } else if input.watchdog_killed {
        TransformOutcome::ResourceLimit
    } else if input.overflow_killed {
        TransformOutcome::OutputInvalid
    } else if input.exit == Exit::Code(SANDBOX_EXIT) {
        TransformOutcome::SandboxUnavailable
    } else if input.cpu >= CPU_LIMIT_SPENT {
        TransformOutcome::CpuLimit
    } else if input.exit == Exit::Signal(SIGKILL) {
        TransformOutcome::Killed
    } else if input.exit != Exit::Code(0) {
        TransformOutcome::ExitNonzero
    } else if input.output_valid {
        TransformOutcome::Ok
    } else {
        TransformOutcome::OutputInvalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLEAN: ClassifyInput = ClassifyInput {
        deadline_killed: false,
        watchdog_killed: false,
        overflow_killed: false,
        exit: Exit::Code(0),
        cpu: Duration::from_secs(1),
        output_valid: true,
    };

    fn with(edit: impl FnOnce(&mut ClassifyInput)) -> TransformOutcome {
        let mut input = CLEAN;
        edit(&mut input);
        classify(&input)
    }

    #[test]
    fn every_rule_in_order() {
        let all = |input: &mut ClassifyInput| {
            input.deadline_killed = true;
            input.watchdog_killed = true;
            input.overflow_killed = true;
            input.exit = Exit::Signal(SIGKILL);
            input.cpu = Duration::from_secs(20);
            input.output_valid = false;
        };
        assert_eq!(with(all), TransformOutcome::Timeout);
        assert_eq!(
            with(|input| {
                all(input);
                input.deadline_killed = false;
            }),
            TransformOutcome::ResourceLimit
        );
        assert_eq!(
            with(|input| {
                all(input);
                input.deadline_killed = false;
                input.watchdog_killed = false;
            }),
            TransformOutcome::OutputInvalid
        );
        assert_eq!(
            with(|input| {
                input.exit = Exit::Code(125);
                input.cpu = Duration::from_secs(20);
            }),
            TransformOutcome::SandboxUnavailable
        );
        // The kernel's SIGKILL and FFmpeg's own SIGXCPU-style exit alike.
        for exit in [Exit::Signal(SIGKILL), Exit::Code(255)] {
            assert_eq!(
                with(|input| {
                    input.exit = exit;
                    input.cpu = CPU_LIMIT_SPENT;
                }),
                TransformOutcome::CpuLimit
            );
        }
        assert_eq!(
            with(|input| input.exit = Exit::Signal(SIGKILL)),
            TransformOutcome::Killed
        );
        for exit in [Exit::Code(1), Exit::Code(255), Exit::Signal(11)] {
            assert_eq!(
                with(|input| input.exit = exit),
                TransformOutcome::ExitNonzero
            );
        }
        assert_eq!(
            with(|input| input.output_valid = false),
            TransformOutcome::OutputInvalid
        );
        assert_eq!(classify(&CLEAN), TransformOutcome::Ok);
        assert_eq!(
            with(|input| input.cpu = Duration::from_millis(19_499)),
            TransformOutcome::Ok
        );
    }

    #[test]
    fn stderr_classes_keep_only_the_class() {
        assert_eq!(StderrClass::of(b""), StderrClass::None);
        assert_eq!(
            StderrClass::of(b"[mov] input: Invalid data found when processing input\n"),
            StderrClass::InvalidData
        );
        for message in [
            &b"Decoder not found"[..],
            b"Unknown decoder 'x'",
            b"could not find codec parameters for stream 0",
        ] {
            assert_eq!(StderrClass::of(message), StderrClass::UnsupportedCodec);
        }
        assert_eq!(StderrClass::of(b"something else"), StderrClass::Other);
        let mut late = vec![b'x'; STDERR_HEAD];
        late.extend_from_slice(b"Invalid data found when processing input");
        assert_eq!(StderrClass::of(&late), StderrClass::Other);
    }

    #[test]
    fn outcomes_have_stable_names() {
        let names: Vec<_> = [
            TransformOutcome::Ok,
            TransformOutcome::Timeout,
            TransformOutcome::CpuLimit,
            TransformOutcome::ResourceLimit,
            TransformOutcome::Killed,
            TransformOutcome::ExitNonzero,
            TransformOutcome::OutputInvalid,
            TransformOutcome::SandboxUnavailable,
        ]
        .iter()
        .map(TransformOutcome::to_string)
        .collect();
        assert_eq!(
            names,
            [
                "ok",
                "timeout",
                "cpu_limit",
                "resource_limit",
                "killed",
                "exit_nonzero",
                "output_invalid",
                "sandbox_unavailable"
            ]
        );
    }
}
