//! Install-command failures keep their command index and nothing else.
use super::support::{adapter_failure, WorkerFailure};
use layer3_adapter::AdapterError;

pub(super) fn install_failure(error: AdapterError) -> WorkerFailure {
    match error {
        AdapterError::Rejected(detail) => {
            let command = detail
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .find(|parts| parts[0] == "command")
                .and_then(|parts| {
                    parts[1]
                        .trim_matches(|c: char| !c.is_ascii_digit())
                        .parse()
                        .ok()
                });
            command.map_or_else(
                || WorkerFailure::terminal_validation("sandbox install command failed"),
                |index: u32| {
                    WorkerFailure::terminal_validation(&format!(
                        "sandbox install command {index} failed"
                    ))
                },
            )
        }
        other => adapter_failure(other),
    }
}
