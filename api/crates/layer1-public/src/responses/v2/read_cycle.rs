use crate::stream_telemetry::CloseReason;
use layer2_core::sandbox::{
    agent_io::{CommandAdmissionError, CommandReceiptView, ObservedResponseReadError},
    AgentReadError, AgentResponseObservationError,
};
use sha2::{Digest, Sha256};
use std::{future::Future, pin::Pin};
use tokio::time::Instant;

type ReadAttempt<'a, T> = Pin<Box<dyn Future<Output = Result<T, ReadFailure>> + Send + 'a>>;

pub(super) async fn read_cycle<C, T>(
    context: &mut C,
    deadline: Instant,
    grant_deadline: Option<Instant>,
    mut read: impl for<'a> FnMut(&'a mut C) -> ReadAttempt<'a, T>,
    mut on_error: impl FnMut(&mut C, &ReadFailure, bool) -> bool,
) -> Result<T, ReadFailure> {
    let timeout = || {
        if grant_deadline.is_some_and(|expires| Instant::now() >= expires) {
            ReadFailure::Expired
        } else {
            ReadFailure::Deadline
        }
    };
    let mut first = true;
    loop {
        let result = if Instant::now() >= deadline {
            Err(timeout())
        } else {
            tokio::time::timeout_at(deadline, read(context))
                .await
                .unwrap_or_else(|_| Err(timeout()))
        };
        let failure = match result {
            Ok(value) if Instant::now() < deadline => return Ok(value),
            Ok(_) => timeout(),
            Err(failure) => failure,
        };
        let available = first && Instant::now() < deadline;
        let retry = on_error(context, &failure, available);
        if !available || !retry {
            return Err(failure);
        }
        first = false;
    }
}

pub(super) enum ReadFailure {
    Command(CommandAdmissionError),
    Header(AgentReadError),
    Source(AgentReadError),
    Release(AgentReadError),
    SourceAndRelease {
        source: AgentReadError,
        release: AgentReadError,
    },
    Receipt,
    Projection,
    Pagination,
    Deadline,
    Expired,
}

impl From<ObservedResponseReadError> for ReadFailure {
    fn from(value: ObservedResponseReadError) -> Self {
        match value {
            ObservedResponseReadError::Command(error) => Self::Command(error),
            ObservedResponseReadError::Header(error) => Self::Header(error),
        }
    }
}

impl From<AgentResponseObservationError> for ReadFailure {
    fn from(value: AgentResponseObservationError) -> Self {
        match value {
            AgentResponseObservationError::Authorization(error) => Self::Header(error),
            AgentResponseObservationError::Source(error) => Self::Source(error),
            AgentResponseObservationError::Release(error) => Self::Release(error),
            AgentResponseObservationError::SourceAndRelease { source, release } => {
                Self::SourceAndRelease { source, release }
            }
        }
    }
}
impl ReadFailure {
    pub fn advanced(&self) -> bool {
        matches!(self, Self::Source(AgentReadError::CaptureAdvanced))
    }

    pub fn close_reason(&self) -> CloseReason {
        match self {
            Self::Command(error) => CloseReason::command(error),
            Self::Header(error) | Self::Source(error) | Self::Release(error) => {
                CloseReason::read(error)
            }
            Self::SourceAndRelease { release, .. } => CloseReason::read(release),
            Self::Deadline => CloseReason::DependencyError,
            Self::Expired => CloseReason::AuthorizationUnavailable,
            Self::Pagination => CloseReason::PaginationRequired,
            Self::Receipt | Self::Projection => CloseReason::ServerError,
        }
    }

    pub fn diagnostic(&self) -> (&'static str, &'static str) {
        match self {
            Self::Command(error) => ("command_read", CloseReason::command(error).label()),
            Self::Header(error) => ("header_read", read_category(error)),
            Self::Source(error) if self.advanced() => ("capture_open", read_category(error)),
            Self::Source(error) => ("source_projection", read_category(error)),
            Self::SourceAndRelease { source, .. } => ("source_projection", read_category(source)),
            Self::Release(error) => ("refresh_authorization", read_category(error)),
            Self::Receipt => ("receipt_check", "receipt_mismatch"),
            Self::Projection => ("source_projection", "invalid_projection"),
            Self::Pagination => ("source_projection", "pagination_required"),
            Self::Deadline => ("refresh", "deadline"),
            Self::Expired => ("refresh_authorization", "expired"),
        }
    }
}

pub(super) fn read_category(error: &AgentReadError) -> &'static str {
    match error {
        AgentReadError::CaptureAdvanced => "capture_advanced",
        AgentReadError::Corrupt(_) => "integrity",
        other => CloseReason::read(other).label(),
    }
}

pub(super) struct FirstFailure {
    correlation: Option<String>,
    recorded: bool,
}

impl FirstFailure {
    pub fn new(receipt: &CommandReceiptView, post: bool) -> Self {
        Self {
            correlation: post.then(|| {
                let digest = Sha256::digest(receipt.command_id.to_string().as_bytes());
                format!("{:x}", digest)[..24].to_owned()
            }),
            recorded: false,
        }
    }

    pub fn record(
        &mut self,
        path: &'static str,
        next: u64,
        failure: (&'static str, &'static str),
        custody: bool,
        retry: &'static str,
    ) {
        if let Some(correlation) = self.correlation.as_ref().filter(|_| !self.recorded) {
            let instance = std::env::var("HOSTNAME")
                .ok()
                .map(|name| format!("{:x}", Sha256::digest(name.as_bytes()))[..24].to_owned());
            self.recorded = true;
            tracing::warn!(
                correlation,
                instance = instance.as_deref().unwrap_or("unknown"),
                source = option_env!("CHRONO_BUILD_SHA").unwrap_or("development"),
                path,
                next,
                stage = failure.0,
                category = failure.1,
                custody,
                retry,
                "Responses v2 POST first observation failure"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, time::Duration};

    fn advanced() -> ReadFailure {
        ReadFailure::Source(AgentReadError::CaptureAdvanced)
    }

    fn custody_lost() -> ReadFailure {
        ReadFailure::Header(AgentReadError::Unavailable("custody ended".into()))
    }

    #[tokio::test]
    async fn capture_and_custody_recovery_share_one_extra_read_in_both_orders() {
        for failures in [
            [advanced(), custody_lost()],
            [custody_lost(), advanced()],
            [advanced(), advanced()],
        ] {
            let mut captures = VecDeque::from(failures);
            let mut permits = Vec::new();
            let result = read_cycle(
                &mut captures,
                Instant::now() + Duration::from_secs(15),
                None,
                |captures| {
                    Box::pin(async { Err::<(), _>(captures.pop_front().expect("third capture")) })
                },
                |_, _, available| {
                    permits.push(available);
                    available
                },
            )
            .await;
            assert!(result.is_err());
            assert_eq!(permits, [true, false]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn recapture_uses_remaining_time_and_does_not_install_after_deadline() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(15);
        let mut calls = 0;
        let result = read_cycle(
            &mut calls,
            deadline,
            None,
            |calls| {
                Box::pin(async {
                    *calls += 1;
                    if *calls == 1 {
                        tokio::time::sleep(Duration::from_secs(14)).await;
                        return Err(advanced());
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok(())
                })
            },
            |_, failure, available| available && failure.advanced(),
        )
        .await;
        assert_eq!(calls, 2);
        assert!(matches!(result, Err(ReadFailure::Deadline)));
        assert_eq!(Instant::now(), deadline);
    }
}
