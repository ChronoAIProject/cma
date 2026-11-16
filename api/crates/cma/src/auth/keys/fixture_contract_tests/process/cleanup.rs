//! Bounded exact-child observation, independent of fixture receipt validity.
use super::{directory::PrivateDirectory, receipt};
use serde::Serialize;
use std::{any::Any, io, time::Duration};
use tokio::{io::AsyncBufRead, process::Child, time::Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Failure {
    WaitError,
    GraceTimeout,
    KillError,
    JoinError,
    JoinTimeout,
    ExitUnsuccessful,
    ReceiptRead,
    ReceiptTimeout,
    ReceiptInvalid,
    LockRemains,
    StateIo,
    Interrupted,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct ExitObservation {
    pub exit_confirmed: bool,
    pub kill_requested: bool,
    pub failures: Vec<Failure>,
}

pub(super) trait ChildControl {
    async fn wait(&mut self) -> io::Result<bool>;
    fn start_kill(&mut self) -> io::Result<()>;
}

impl ChildControl for Child {
    async fn wait(&mut self) -> io::Result<bool> {
        Child::wait(self).await.map(|status| status.success())
    }
    fn start_kill(&mut self) -> io::Result<()> {
        Child::start_kill(self)
    }
}

pub(super) async fn observe_exit(
    child: &mut impl ChildControl,
    grace: Instant,
    deadline: Instant,
) -> ExitObservation {
    let mut observed = ExitObservation::default();
    match tokio::time::timeout_at(grace, child.wait()).await {
        Ok(Ok(success)) => {
            observed.exit_confirmed = true;
            if !success {
                observed.failures.push(Failure::ExitUnsuccessful);
            }
            return observed;
        }
        Ok(Err(_)) => observed.failures.push(Failure::WaitError),
        Err(_) => observed.failures.push(Failure::GraceTimeout),
    }
    // start_kill only targets the owned Child; it cannot establish exit. A
    // failed signal still gets a bounded wait in case the child exited itself.
    observed.kill_requested = true;
    if child.start_kill().is_err() {
        observed.failures.push(Failure::KillError);
    }
    match tokio::time::timeout_at(deadline, child.wait()).await {
        Ok(Ok(_)) => observed.exit_confirmed = true,
        Ok(Err(_)) => observed.failures.push(Failure::JoinError),
        Err(_) => observed.failures.push(Failure::JoinTimeout),
    }
    observed
}

pub(super) async fn close(
    child: &mut impl ChildControl,
    output: &mut (impl AsyncBufRead + Unpin),
    directory: &mut PrivateDirectory,
) -> Result<(), Failure> {
    let start = Instant::now();
    let deadline = start + Duration::from_secs(6);
    // Drain stdout concurrently: a full closed receipt must not prevent the
    // child from exiting. All asynchronous cleanup shares this absolute bound.
    let (mut observed, closed) = tokio::join!(
        observe_exit(child, start + Duration::from_secs(5), deadline),
        receipt::read_closed(output, deadline),
    );
    let closed = match closed {
        Ok(receipt) => Some(receipt),
        Err(failure) => {
            observed.failures.push(failure);
            None
        }
    };
    directory.finish(observed, closed)
}

type Panic = Box<dyn Any + Send>;

pub(super) fn finish_test(body: Result<(), Panic>, cleanup: Result<Result<(), Failure>, Panic>) {
    // Fixed error codes exclude child stdout, provider payloads and tokens.
    match &cleanup {
        Ok(Err(failure)) => eprintln!("provider fixture cleanup failed: {failure:?}"),
        Err(_) => eprintln!("provider fixture cleanup panicked; private state retained"),
        Ok(Ok(())) => {}
    }
    if let Err(panic) = body {
        std::panic::resume_unwind(panic);
    }
    match cleanup {
        Ok(result) => result.expect("provider fixture cleanup must complete"),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
