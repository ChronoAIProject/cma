use std::future::Future;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::{RuntimeRecoveryCommand, ARGUMENT_LIMIT, IO_LIMIT};
use crate::agent_sandbox::PodBinding;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Command,
    SandboxIdentity,
    ContainerIdentity,
    ChangedIdentity,
    Write,
    InputEof,
    Output,
    OutputLimit,
    Exit,
    Join,
}

impl Failure {
    pub(super) fn into_adapter(self) -> crate::AdapterError {
        match self {
            Self::Command => {
                super::rejected("runtime recovery command command exceeds its contract")
            }
            Self::SandboxIdentity => {
                super::rejected("runtime recovery command Sandbox identity mismatch")
            }
            Self::ContainerIdentity => {
                super::rejected("runtime recovery command container identity is missing")
            }
            Self::ChangedIdentity => {
                super::rejected("Sandbox Pod changed during runtime recovery command")
            }
            Self::Write => super::unavailable("runtime recovery command write failed"),
            Self::InputEof => super::unavailable("runtime recovery command EOF failed"),
            Self::Output => super::unavailable("runtime recovery command output failed"),
            Self::OutputLimit => {
                super::rejected("runtime recovery command output exceeds its bound")
            }
            Self::Exit => super::unavailable("runtime recovery command command did not succeed"),
            Self::Join => super::unavailable("runtime recovery command stream failed"),
        }
    }
}

pub(super) fn validate_command(command: &RuntimeRecoveryCommand) -> Result<(), Failure> {
    if !command.program.starts_with('/')
        || command.program.as_bytes().contains(&0)
        || command.program.len() > ARGUMENT_LIMIT
        || command.args.len() > 64
        || command.args.iter().any(|arg| arg.as_bytes().contains(&0))
        || command
            .args
            .iter()
            .try_fold(command.program.len(), |total, arg| {
                total.checked_add(arg.len())
            })
            .is_none_or(|total| total > ARGUMENT_LIMIT)
        || command.input.len() > IO_LIMIT
    {
        return Err(Failure::Command);
    }
    Ok(())
}

pub(super) fn validate_binding(
    expected: (&str, &str),
    binding: &PodBinding,
) -> Result<(), Failure> {
    if binding.sandbox_name != expected.0 || binding.sandbox_uid != expected.1 {
        return Err(Failure::SandboxIdentity);
    }
    if binding
        .runtime_container_id
        .as_deref()
        .is_none_or(str::is_empty)
        || binding.runtime_restart_count.is_none_or(|count| count < 0)
    {
        return Err(Failure::ContainerIdentity);
    }
    Ok(())
}

pub(super) fn validate_final_binding(
    before: &PodBinding,
    after: &PodBinding,
) -> Result<(), Failure> {
    if before != after {
        return Err(Failure::ChangedIdentity);
    }
    Ok(())
}

// The caller supplies buffer storage before polling. An admitted caller must
// also own the streams, status and join future through their physical teardown.
pub(super) async fn exchange_io<W, R, S, J>(
    mut stdin: W,
    stdout: R,
    input: &[u8],
    status: S,
    join: impl FnOnce() -> J,
    output: Vec<u8>,
) -> Result<Vec<u8>, Failure>
where
    W: AsyncWrite + Unpin,
    R: AsyncRead + Unpin,
    S: Future<Output = bool>,
    J: Future<Output = bool>,
{
    let send = async {
        stdin.write_all(input).await.map_err(|_| Failure::Write)?;
        stdin.shutdown().await.map_err(|_| Failure::InputEof)?;
        drop(stdin);
        Ok(())
    };
    let exit = async {
        if !status.await {
            return Err(Failure::Exit);
        }
        Ok(())
    };
    let ((), output, ()) = tokio::try_join!(send, bounded_output(stdout, output), exit)?;
    if !join().await {
        return Err(Failure::Join);
    }
    Ok(output)
}

async fn bounded_output(
    reader: impl AsyncRead + Unpin,
    mut bytes: Vec<u8>,
) -> Result<Vec<u8>, Failure> {
    reader
        .take(IO_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|_| Failure::Output)?;
    if bytes.len() > IO_LIMIT {
        return Err(Failure::OutputLimit);
    }
    Ok(bytes)
}
